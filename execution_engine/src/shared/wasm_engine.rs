//! Preprocessing of Wasm modules.
use casper_types::{
    account::AccountHash,
    api_error,
    bytesrepr::{self, Bytes, FromBytes, ToBytes},
    ApiError, CLValue, Gas, Key, ProtocolVersion, StoredValue, URef, U512,
};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use parity_wasm::elements::{self, MemorySection, Section};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{hash_map::Entry, HashMap},
    error::Error,
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::Write,
    path::Path,
    rc::Rc,
    time::Instant,
};
use thiserror::Error;

const DEFAULT_GAS_MODULE_NAME: &str = "env";

use parity_wasm::elements::Module as WasmiModule;
use wasmi::{ImportsBuilder, MemoryRef, ModuleInstance, ModuleRef};
use wasmtime::{
    AsContext, AsContextMut, Caller, Extern, ExternType, InstanceAllocationStrategy, Memory,
    MemoryType, StoreContextMut, Trap,
};

use crate::{
    core::{
        execution,
        resolvers::{create_module_resolver, memory_resolver::MemoryResolver},
        runtime::Runtime,
    },
    storage::global_state::StateReader,
};

use super::wasm_config::WasmConfig;

#[derive(Debug, Copy, Clone, PartialEq, Eq, FromPrimitive, ToPrimitive, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Runs Wasm modules in interpreted mode.
    Interpreted = 1,
    /// Runs Wasm modules in a compiled mode.
    Compiled = 2,
}

impl ToBytes for ExecutionMode {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        self.to_u32().unwrap().to_bytes()
    }

    fn serialized_length(&self) -> usize {
        self.to_u32().unwrap().serialized_length()
    }
}

impl FromBytes for ExecutionMode {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (execution_mode, rem) = u32::from_bytes(bytes)?;
        let execution_mode =
            ExecutionMode::from_u32(execution_mode).ok_or(bytesrepr::Error::Formatting)?;
        Ok((execution_mode, rem))
    }
}

impl Distribution<ExecutionMode> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> ExecutionMode {
        match rng.gen::<bool>() {
            false => ExecutionMode::Interpreted,
            true => ExecutionMode::Compiled,
        }
    }
}

fn deserialize_interpreted(wasm_bytes: &[u8]) -> Result<WasmiModule, PreprocessingError> {
    parity_wasm::elements::deserialize_buffer(wasm_bytes).map_err(PreprocessingError::from)
}

/// Statically dispatched Wasm module wrapper for different implementations
/// NOTE: Probably at this stage it can hold raw wasm bytes without being an enum, and the decision
/// can be made later
#[derive(Clone)]
pub enum Module {
    Interpreted(WasmiModule),
    Compiled(Vec<u8>), /* AOT
                        * Compiled(wasmtime::Module), */
}

impl fmt::Debug for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interpreted(arg0) => f.debug_tuple("Interpreted").field(arg0).finish(),
            Self::Compiled(arg0) => f.debug_tuple("Compiled").finish(),
        }
    }
}

impl Module {
    pub fn try_into_interpreted(self) -> Result<WasmiModule, Self> {
        if let Self::Interpreted(v) = self {
            Ok(v)
        } else {
            Err(self)
        }
    }

    /// Consumes self, and returns an [`WasmiModule`] regardless of an implementation.
    ///
    /// Such module can be useful for performing optimizations. To create a [`Module`] back use
    /// appropriate [`WasmEngine`] method.
    pub fn into_interpreted(self) -> WasmiModule {
        match self {
            Module::Interpreted(parity_wasm) => parity_wasm,
            Module::Compiled(_) => {
                todo!("serialize compiled module")
            }
        }
    }

    // pub fn try_into_compiled(self) -> Result<wasmtime::Module, Self> {
    //     if let Self::Compiled(v) = self {
    //         Ok(v)
    //     } else {
    //         Err(self)
    //     }
    // }
}

impl From<WasmiModule> for Module {
    fn from(wasmi_module: WasmiModule) -> Self {
        Self::Interpreted(wasmi_module)
    }
}

// impl From<wasmtime::Module> for Module {
//     fn from(wasmtime_module: wasmtime::Module) -> Self {
//         Self::Compiled(wasmtime_module)
//     }
// \}

/// Common error type adapter for all Wasm engines supported.
#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    WasmiError(#[from] wasmi::Error),
    #[error(transparent)]
    WasmtimeError(#[from] wasmtime::Trap),
    #[error("{0}")]
    Other(String),
}

impl From<String> for RuntimeError {
    fn from(string: String) -> Self {
        Self::Other(string)
    }
}

impl RuntimeError {
    /// Extracts the executor error from a runtime Wasm trap.
    pub fn as_execution_error(&self) -> Option<&execution::Error> {
        match self {
            RuntimeError::WasmiError(wasmi_error) => wasmi_error
                .as_host_error()
                .and_then(|host_error| host_error.downcast_ref::<execution::Error>()),
            RuntimeError::WasmtimeError(wasmtime_trap) => {
                let src = wasmtime_trap.source()?;
                let error = src.downcast_ref::<execution::Error>()?;
                Some(error)
            }
            RuntimeError::Other(_) => None,
        }
    }
}

impl Into<String> for RuntimeError {
    fn into(self) -> String {
        self.to_string()
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RuntimeValue {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl From<wasmi::RuntimeValue> for RuntimeValue {
    fn from(runtime_value: wasmi::RuntimeValue) -> Self {
        match runtime_value {
            wasmi::RuntimeValue::I32(value) => RuntimeValue::I32(value),
            wasmi::RuntimeValue::I64(value) => RuntimeValue::I64(value),
            // NOTE: Why wasmi implements F32/F64 newtypes? Is F64->f64 conversion safe even though
            // they claim they're compatible with rust's IEEE 754-2008?
            wasmi::RuntimeValue::F32(value) => RuntimeValue::F32(value.to_float()),
            wasmi::RuntimeValue::F64(value) => RuntimeValue::F64(value.to_float()),
        }
    }
}

impl Into<wasmi::RuntimeValue> for RuntimeValue {
    fn into(self) -> wasmi::RuntimeValue {
        match self {
            RuntimeValue::I32(value) => wasmi::RuntimeValue::I32(value),
            RuntimeValue::I64(value) => wasmi::RuntimeValue::I64(value),
            RuntimeValue::F32(value) => wasmi::RuntimeValue::F32(value.into()),
            RuntimeValue::F64(value) => wasmi::RuntimeValue::F64(value.into()),
        }
    }
}

/// Warmed up instance of a wasm module used for execution.
#[derive(Clone)]
pub enum Instance {
    Interpreted(ModuleRef, MemoryRef),
    // NOTE: Instance should contain wasmtime::Instance instead but we need to hold Store that has
    // a lifetime and a generic R
    Compiled(wasmtime::Module),
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interpreted(arg0, arg1) => f
                .debug_tuple("Interpreted")
                .field(arg0)
                .field(arg1)
                .finish(),
            Self::Compiled(_arg0) => f.debug_tuple("Compiled").finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstanceRef(Rc<Instance>);

impl From<Instance> for InstanceRef {
    fn from(instance: Instance) -> Self {
        InstanceRef(Rc::new(instance))
    }
}

pub trait FunctionContext {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError>;
    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), RuntimeError>;
}

pub struct WasmiAdapter {
    memory: wasmi::MemoryRef,
}

impl WasmiAdapter {
    pub fn new(memory: wasmi::MemoryRef) -> Self {
        Self { memory }
    }
}

impl FunctionContext for WasmiAdapter {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError> {
        Ok(self.memory.get(offset, size)?)
    }

    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), RuntimeError> {
        self.memory.set(offset, data)?;
        Ok(())
    }
}
/// Wasm caller object passed as an argument for each.
pub struct WasmtimeAdapter<'a> {
    data: &'a mut [u8],
}

impl<'a> FunctionContext for WasmtimeAdapter<'a> {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError> {
        let mut buffer = vec![0; size];

        let slice = self
            .data
            .get(offset as usize..)
            .and_then(|s| s.get(..buffer.len()))
            .ok_or_else(|| Trap::new("memory access"))?;
        buffer.copy_from_slice(slice);
        Ok(buffer)
    }

    fn memory_write(&mut self, offset: u32, buffer: &[u8]) -> Result<(), RuntimeError> {
        self.data
            .get_mut(offset as usize..)
            .and_then(|s| s.get_mut(..buffer.len()))
            .ok_or_else(|| Trap::new("memory access"))?
            .copy_from_slice(buffer);
        Ok(())
    }
}

impl Instance {
    pub fn interpreted_memory(&self) -> MemoryRef {
        match self {
            Instance::Interpreted(_, memory_ref) => memory_ref.clone(),
            Instance::Compiled(_) => unreachable!("available only from wasmi externals"),
        }
    }
    /// Invokes exported function
    pub fn invoke_export<R>(
        &self,
        wasm_engine: &WasmEngine,
        func_name: &str,
        args: Vec<RuntimeValue>,
        runtime: &mut Runtime<R>,
    ) -> Result<Option<RuntimeValue>, RuntimeError>
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<execution::Error>,
    {
        match self.clone() {
            Instance::Interpreted(module_ref, memory_ref) => {
                let wasmi_args: Vec<wasmi::RuntimeValue> =
                    args.into_iter().map(|value| value.into()).collect();
                // get the runtime value
                let wasmi_result =
                    module_ref.invoke_export(func_name, wasmi_args.as_slice(), runtime)?;
                // wasmi's runtime value into our runtime value
                let result = wasmi_result.map(RuntimeValue::from);
                Ok(result)
            }
            Instance::Compiled(mut compiled_module) => {
                let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine, runtime);

                let mut linker = wasmtime::Linker::new(&wasm_engine.compiled_engine);

                let memory_import = compiled_module
                    .imports()
                    .filter_map(|import| {
                        if (import.module(), import.name()) == ("env", Some("memory")) {
                            Some(import.ty())
                        } else {
                            None
                        }
                    })
                    .next();

                let memory_type = match memory_import {
                    Some(ExternType::Memory(memory)) => memory,
                    Some(unknown_extern) => panic!("unexpected extern {:?}", unknown_extern),
                    None => MemoryType::new(1, Some(wasm_engine.wasm_config().max_memory)),
                };

                debug_assert_eq!(
                    memory_type.maximum(),
                    Some(wasm_engine.wasm_config().max_memory as u64)
                );

                let memory = wasmtime::Memory::new(&mut store, memory_type).unwrap();

                linker
                    .define("env", "memory", wasmtime::Extern::from(memory))
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_revert",
                        |mut caller: Caller<'_, &mut Runtime<R>>, param: u32| {
                            let mut runtime = caller.data_mut();
                            runtime.casper_revert(param)?;
                            Ok(()) //unreachable
                        },
                    )
                    .unwrap();
                linker
                    .func_wrap(
                        "env",
                        "gas",
                        |mut caller: Caller<'_, &mut Runtime<R>>, param: u32| {
                            let mut runtime = caller.data_mut();
                            runtime.gas(Gas::new(U512::from(param)))?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_new_uref",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         uref_ptr,
                         value_ptr,
                         value_size| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            // runtime.casper_new_uref(
                            //     function_context,
                            //     uref_ptr,
                            //     value_ptr,
                            //     value_size,
                            // )?;
                            // FunctionIndex::NewFuncIndex
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_create_purse",
                        |mut caller: Caller<'_, &mut Runtime<R>>, dest_ptr: u32, dest_size: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            let ret =runtime.casper_create_purse(function_context, dest_ptr, dest_size)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_write",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         value_ptr: u32,
                         value_size: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            runtime.casper_write(
                                function_context,
                                key_ptr,
                                key_size,
                                value_ptr,
                                value_size,
                            )?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_main_purse",
                        |mut caller: Caller<'_, &mut Runtime<R>>, dest_ptr: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            runtime.casper_get_main_purse(function_context, dest_ptr)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_named_arg_size",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         name_ptr: u32,
                         name_size: u32,
                         size_ptr: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };

                            // let ret =
                            let ret = runtime.casper_get_named_arg_size(function_context, name_ptr, name_size, size_ptr)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_named_arg",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         name_ptr: u32,
                         name_size: u32,
                         dest_ptr: u32,
                         dest_size: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            let ret = runtime.casper_get_named_arg(function_context, name_ptr, name_size, dest_ptr, dest_size)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_transfer_to_account",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         amount_ptr: u32,
                         amount_size: u32,
                         id_ptr: u32,
                         id_size: u32,
                         result_ptr: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            let ret = runtime.casper_transfer_to_account(function_context, key_ptr,
                                key_size,
                                amount_ptr,
                                amount_size,
                                id_ptr,
                                id_size,
                                result_ptr)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_put_key",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         name_ptr,
                         name_size,
                         key_ptr,
                         key_size| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };

                            runtime.casper_put_key(function_context,
                                name_ptr,
                                name_size,
                                key_ptr,
                                key_size)?;

                                
                            Ok(())
                        },
                    )
                    .unwrap();

                #[cfg(feature = "test-support")]
                linker
                    .func_wrap(
                        "env",
                        "casper_print",
                        |mut caller: Caller<'_, &mut Runtime<R>>, text_ptr: u32, text_size: u32| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            runtime.casper_print(function_context, text_ptr, text_size)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_transfer_from_purse_to_purse",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         source_ptr,
                         source_size,
                         target_ptr,
                         target_size,
                         amount_ptr,
                         amount_size,
                         id_ptr,
                         id_size| {
                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let (data, runtime) = mem.data_and_store_mut(&mut caller);
                            let function_context = WasmtimeAdapter { data };
                            let ret = runtime.casper_transfer_from_purse_to_purse(function_context,
                                source_ptr,
                         source_size,
                         target_ptr,
                         target_size,
                         amount_ptr,
                         amount_size,
                         id_ptr,
                         id_size)?;
                            Ok(api_error::i32_from(ret))
                         }
                    )
                    .unwrap();

                let instance = linker
                    .instantiate(&mut store, &compiled_module)
                    .expect("should instantiate");
                let exported_func = instance
                    .get_typed_func::<(), (), _>(&mut store, func_name)
                    .expect("should get typed func");
                exported_func
                    .call(&mut store, ())
                    .map_err(RuntimeError::from)?;
                Ok(Some(RuntimeValue::I64(0)))
            }
        }
    }
}

/// An error emitted by the Wasm preprocessor.
#[derive(Debug, Clone, Error)]
pub enum PreprocessingError {
    /// Unable to deserialize Wasm bytes.
    #[error("Deserialization error: {0}")]
    Deserialize(String),
    /// Found opcodes forbidden by gas rules.
    #[error(
        "Encountered operation forbidden by gas rules. Consult instruction -> metering config map"
    )]
    OperationForbiddenByGasRules,
    /// Stack limiter was unable to instrument the binary.
    #[error("Stack limiter error")]
    StackLimiter,
    /// Wasm bytes is missing memory section.
    #[error("Memory section should exist")]
    MissingMemorySection,
}

impl From<elements::Error> for PreprocessingError {
    fn from(error: elements::Error) -> Self {
        PreprocessingError::Deserialize(error.to_string())
    }
}

// impl Display for PreprocessingError {
//     fn fmt(&self, f: &mut Formatter) -> fmt::Result {
//         match self {
//             PreprocessingError::Deserialize(error) => write!(f, ", error),
//             PreprocessingError::OperationForbiddenByGasRules => write!(f, ""),
//             PreprocessingError::StackLimiter => write!(f, "Stack limiter error"),
//             PreprocessingError::MissingMemorySection => write!(f, "Memory section should exist"),
//         }
//     }
// }

/// Checks if given wasm module contains a memory section.
fn memory_section(module: &WasmiModule) -> Option<&MemorySection> {
    for section in module.sections() {
        if let Section::Memory(section) = section {
            return Some(section);
        }
    }
    None
}

#[derive(Clone)]
struct WasmtimeEngine(wasmtime::Engine);

impl fmt::Debug for WasmtimeEngine {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WasmtimeEngine").finish()
    }
}

impl std::ops::Deref for WasmtimeEngine {
    type Target = wasmtime::Engine;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Wasm preprocessor.
#[derive(Debug, Clone)]
pub struct WasmEngine {
    wasm_config: WasmConfig,
    execution_mode: ExecutionMode,
    compiled_engine: WasmtimeEngine,
    cache: RefCell<HashMap<Vec<u8>, Vec<u8>>>,
}

fn setup_wasmtime_caching(cache_path: &Path, config: &mut wasmtime::Config) -> Result<(), String> {
    let wasmtime_cache_root = cache_path.join("wasmtime");
    fs::create_dir_all(&wasmtime_cache_root)
        .map_err(|err| format!("cannot create the dirs to cache: {:?}", err))?;

    // Canonicalize the path after creating the directories.
    let wasmtime_cache_root = wasmtime_cache_root
        .canonicalize()
        .map_err(|err| format!("failed to canonicalize the path: {:?}", err))?;

    // Write the cache config file
    let cache_config_path = wasmtime_cache_root.join("cache-config.toml");

    let config_content = format!(
        "\
[cache]
enabled = true
directory = \"{cache_dir}\"
",
        cache_dir = wasmtime_cache_root.display()
    );
    fs::write(&cache_config_path, config_content)
        .map_err(|err| format!("cannot write the cache config: {:?}", err))?;

    config
        .cache_config_load(cache_config_path)
        .map_err(|err| format!("failed to parse the config: {:?}", err))?;

    Ok(())
}

fn new_compiled_engine(wasm_config: &WasmConfig) -> WasmtimeEngine {
    let mut config = wasmtime::Config::new();
    setup_wasmtime_caching(&Path::new("/tmp/wasmtime_test"), &mut config)
        .expect("should setup wasmtime cache path");
    config.cranelift_opt_level(wasmtime::OptLevel::SpeedAndSize);
    // config.async_support(false);
    config.wasm_reference_types(false);
    config.wasm_simd(false);
    config.wasm_bulk_memory(false);
    config.wasm_multi_value(false);
    config.wasm_multi_memory(false);
    config.wasm_module_linking(false);
    config.wasm_threads(false);

    config
        .max_wasm_stack(wasm_config.max_stack_height as usize)
        .expect("should set max stack");

    // TODO: Tweak more
    let wasmtime_engine = wasmtime::Engine::new(&config).expect("should create new engine");
    WasmtimeEngine(wasmtime_engine)
}

impl WasmEngine {
    /// Creates a new instance of the preprocessor.
    pub fn new(wasm_config: WasmConfig) -> Self {
        Self {
            wasm_config,
            execution_mode: wasm_config.execution_mode,
            compiled_engine: new_compiled_engine(&wasm_config),
            cache: Default::default(),
        }
    }

    pub fn execution_mode(&self) -> &ExecutionMode {
        &self.execution_mode
    }

    pub fn execution_mode_mut(&mut self) -> &mut ExecutionMode {
        &mut self.execution_mode
    }

    /// Preprocesses Wasm bytes and returns a module.
    ///
    /// This process consists of a few steps:
    /// - Validate that the given bytes contain a memory section, and check the memory page limit.
    /// - Inject gas counters into the code, which makes it possible for the executed Wasm to be
    ///   charged for opcodes; this also validates opcodes and ensures that there are no forbidden
    ///   opcodes in use, such as floating point opcodes.
    /// - Ensure that the code has a maximum stack height.
    ///
    /// In case the preprocessing rules can't be applied, an error is returned.
    /// Otherwise, this method returns a valid module ready to be executed safely on the host.
    pub fn preprocess(&self, module_bytes: &[u8]) -> Result<Module, PreprocessingError> {
        let module = deserialize_interpreted(module_bytes)?;

        let module = pwasm_utils::inject_gas_counter(
            module,
            &self.wasm_config.opcode_costs().to_set(),
            DEFAULT_GAS_MODULE_NAME,
        )
        .map_err(|_| PreprocessingError::OperationForbiddenByGasRules)?;

        match self.execution_mode {
            ExecutionMode::Interpreted => {
                if memory_section(&module).is_none() {
                    // `pwasm_utils::externalize_mem` expects a memory section to exist in the
                    // module, and panics otherwise.
                    return Err(PreprocessingError::MissingMemorySection);
                }
                let module =
                    pwasm_utils::externalize_mem(module, None, self.wasm_config.max_memory);

                let module =
                    stack_height::inject_limiter(module, self.wasm_config.max_stack_height)
                        .map_err(|_| PreprocessingError::StackLimiter)?;

                Ok(module.into())
            }
            ExecutionMode::Compiled => {
                let preprocessed_wasm_bytes =
                    parity_wasm::serialize(module).expect("preprocessed wasm to bytes");

                // aot compile
                let precompiled_bytes = self.precompile(module_bytes).unwrap();
                // let wasmtime_module =
                //     wasmtime::Module::new(&self.compiled_engine, &preprocessed_wasm_bytes)
                //         .expect("should process");
                Ok(Module::Compiled(precompiled_bytes.to_owned()))

                // let compiled_module =
                //             wasmtime::Module::new(&self.compiled_engine,
                // &preprocessed_wasm_bytes)                 .map_err(|e|
                // PreprocessingError::Deserialize(e.to_string()))?;
                //     Ok(compiled_module.into())
            }
        }
        // let module = deserialize_interpreted(module_bytes)?;
    }

    /// Get a reference to the wasm engine's wasm config.
    pub fn wasm_config(&self) -> &WasmConfig {
        &self.wasm_config
    }

    /// Creates module specific for execution mode.
    pub fn module_from_bytes(&self, wasm_bytes: &[u8]) -> Result<Module, PreprocessingError> {
        let module = match self.execution_mode {
            ExecutionMode::Interpreted => {
                let parity_module = parity_wasm::deserialize_buffer::<WasmiModule>(wasm_bytes)
                    .map_err(PreprocessingError::from)?;
                Module::Interpreted(parity_module)
            }
            ExecutionMode::Compiled => {
                // aot compile
                let precompiled_bytes = self.precompile(wasm_bytes).unwrap();
                // self.compiled_engine.precompile_module(&wasm_bytes).expect("should preprocess");
                // let module = wasmtime::Module::new(&self.compiled_engine, wasm_bytes).unwrap();
                Module::Compiled(precompiled_bytes.to_owned())
                // let compiled_module =
                //             wasmtime::Module::new(&self.compiled_engine,
                // &preprocessed_wasm_bytes)                 .map_err(|e|
                // PreprocessingError::Deserialize(e.to_string()))?;
                //     Ok(compiled_module.into())
                // let compiled_module = wasmtime::Module::new(&self.compiled_engine, wasm_bytes)
                //     .map_err(|e| PreprocessingError::Deserialize(e.to_string()))?;
                // Module::Compiled(compiled_module)
            }
        };
        Ok(module)
    }

    /// Get a reference to the wasm engine's compiled engine.
    pub fn compiled_engine(&self) -> &wasmtime::Engine {
        &self.compiled_engine
    }
    /// Creates an WASM module instance and a memory instance.
    ///
    /// This ensures that a memory instance is properly resolved into a pre-allocated memory area,
    /// and a host function resolver is attached to the module.
    ///
    /// The WASM module is also validated to not have a "start" section as we currently don't
    /// support running it.
    ///
    /// Both [`ModuleRef`] and a [`MemoryRef`] are ready to be executed.
    pub fn instance_and_memory(
        &self,
        wasm_module: Module,
        protocol_version: ProtocolVersion,
    ) -> Result<Instance, execution::Error> {
        // match wasm_engine.execution_mode() {
        match wasm_module {
            Module::Interpreted(wasmi_module) => {
                // let wasmi_module = wasm_module.try_into_interpreted().expect("expected
                // interpreted wasm module");
                let module = wasmi::Module::from_parity_wasm_module(wasmi_module)?;
                let resolver = create_module_resolver(protocol_version, self.wasm_config())?;
                let mut imports = ImportsBuilder::new();
                imports.push_resolver("env", &resolver);
                let not_started_module = ModuleInstance::new(&module, &imports)?;
                if not_started_module.has_start() {
                    return Err(execution::Error::UnsupportedWasmStart);
                }
                let instance = not_started_module.not_started_instance().clone();
                let memory = resolver.memory_ref()?;
                Ok(Instance::Interpreted(instance, memory))
            }
            Module::Compiled(compiled_module) => {
                // aot compile
                // let precompiled_bytes =
                // self.compiled_engine.precompile_module(&preprocessed_wasm_bytes).expect("should
                // preprocess"); Ok(Module::Compiled(precompiled_bytes))

                // todo!("compiled mode")
                // let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine(), ());
                // let instance = wasmtime::Instance::new(&mut store, &compiled_module,
                // &[]).expect("should create compiled module");

                let compiled_module = self.deserialize_compiled(&compiled_module)?;
                Ok(Instance::Compiled(compiled_module))
            }
        }
    }

    fn deserialize_compiled(&self, bytes: &[u8]) -> Result<wasmtime::Module, execution::Error> {
        let compiled_module =
            unsafe { wasmtime::Module::deserialize(&self.compiled_engine(), bytes) }.unwrap();
        Ok(compiled_module)
    }

    fn precompile(&self, bytes: &[u8]) -> Result<Vec<u8>, execution::Error> {
        let mut cache = self.cache.borrow_mut();
        let bytes = match cache.entry(bytes.to_vec()) {
            Entry::Occupied(o) => o.get().clone(),
            Entry::Vacant(v) => {
                let start = Instant::now();
                let precompiled_bytes = self
                    .compiled_engine
                    .precompile_module(bytes)
                    .expect("should preprocess");
                let stop = start.elapsed();
                eprintln!("precompiled {} bytes in {:?}", bytes.len(), stop);
                v.insert(precompiled_bytes).clone()
            }
        };
        // );
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasmtime_trap_recover() {
        let error = execution::Error::Revert(ApiError::User(100));
        let trap: wasmtime::Trap = wasmtime::Trap::from(error);
        let runtime_error = RuntimeError::from(trap);
        let recovered = runtime_error
            .as_execution_error()
            .expect("should have error");
        assert!(matches!(
            recovered,
            execution::Error::Revert(ApiError::User(100))
        ));
    }
}
