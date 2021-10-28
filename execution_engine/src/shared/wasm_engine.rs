//! Preprocessing of Wasm modules.
use casper_types::{
    account::{self, AccountHash},
    api_error,
    bytesrepr::{self, Bytes, FromBytes, ToBytes},
    contracts::ContractPackageStatus,
    ApiError, CLValue, ContractPackageHash, ContractVersion, Gas, Key, ProtocolVersion,
    StoredValue, URef, U512,
};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use once_cell::sync::Lazy;
use parity_wasm::elements::{self, MemorySection, Section};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::{hash_map::Entry, HashMap},
    error::Error,
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::Write,
    path::Path,
    rc::Rc,
    sync::{Mutex, Once},
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
        resolvers::{
            create_module_resolver, memory_resolver::MemoryResolver,
            v1_function_index::FunctionIndex,
        },
        runtime::{
            scoped_instrumenter::{self, ScopedInstrumenter},
            Runtime,
        },
    },
    shared::host_function_costs,
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
    /// TODO: We might not need it anymore. We use "do nothing" bytes for replacing wasm modules
    Noop,
    Interpreted(WasmiModule),
    Compiled {
        /// Used to carry on original Wasm module for easy further processing.
        wasmi_module: WasmiModule,
        /// Ahead of time compiled artifact.
        compiled_artifact: Vec<u8>,
    },
}

impl fmt::Debug for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noop => f.debug_tuple("Noop").finish(),
            Self::Interpreted(arg0) => f.debug_tuple("Interpreted").field(arg0).finish(),
            Self::Compiled { .. } => f.debug_tuple("Compiled").finish(),
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
            Module::Noop => unimplemented!("Attempting to get interpreted module from noop"),
            Module::Interpreted(parity_wasm) => parity_wasm,
            Module::Compiled { wasmi_module, .. } => wasmi_module,
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
    // TODO: We might not need it. Actually trying to execute a noop is a programming error.
    Noop,
    Interpreted(ModuleRef, MemoryRef),
    // NOTE: Instance should contain wasmtime::Instance instead but we need to hold Store that has
    // a lifetime and a generic R
    Compiled {
        /// Raw Wasmi module used only for further processing.
        module: WasmiModule,
        /// This is compiled module used for execution.
        compiled_module: wasmtime::Module,
    },
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noop => f.debug_tuple("Noop").finish(),
            Self::Interpreted(arg0, arg1) => f
                .debug_tuple("Interpreted")
                .field(arg0)
                .field(arg1)
                .finish(),
            Self::Compiled { .. } => f.debug_tuple("Compiled").finish(),
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

fn caller_adapter_and_runtime<'b, 'a: 'b, 'c, 'd: 'c, R>(
    caller: &'c mut Caller<'d, &'b mut Runtime<'a, R>>,
) -> (WasmtimeAdapter<'c>, &'c mut Runtime<'a, R>) {
    let mem = caller.data().wasmtime_memory;
    let (data, runtime) = mem
        .expect("Memory should have been initialized.")
        .data_and_store_mut(caller);
    (WasmtimeAdapter { data }, runtime)
}

impl Instance {
    pub fn interpreted_memory(&self) -> MemoryRef {
        match self {
            Instance::Noop => unimplemented!("Unable to get interpreted memory from noop module"),
            Instance::Interpreted(_, memory_ref) => memory_ref.clone(),
            Instance::Compiled { .. } => unreachable!("available only from wasmi externals"),
        }
    }
    /// Invokes exported function
    pub fn invoke_export<'a, R>(
        &self,
        wasm_engine: &WasmEngine,
        func_name: &str,
        args: Vec<RuntimeValue>,
        runtime: &mut Runtime<'a, R>,
    ) -> Result<Option<RuntimeValue>, RuntimeError>
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<execution::Error>,
    {
        match self.clone() {
            Instance::Noop => {
                unimplemented!("Unable to execute export {} on noop instance", func_name)
            }
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
            Instance::Compiled {
                module,
                mut compiled_module,
            } => {
                let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine, runtime);

                let mut linker = wasmtime::Linker::new(&wasm_engine.compiled_engine);

                let memory_import = compiled_module.imports().find_map(|import| {
                    if (import.module(), import.name()) == ("env", Some("memory")) {
                        Some(import.ty())
                    } else {
                        None
                    }
                });

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
                store.data_mut().wasmtime_memory = Some(memory);

                linker
                    .define("env", "memory", wasmtime::Extern::from(memory))
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_read_value",
                        |mut caller: Caller<&mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.read(
                                function_context,
                                key_ptr,
                                key_size,
                                output_size_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_add",
                        |mut caller: Caller<&mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         value_ptr: u32,
                         value_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.casper_add(
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
                        "casper_revert",
                        |mut caller: Caller<&mut Runtime<R>>, param: u32| {
                            caller.data_mut().casper_revert(param)?;
                            Ok(()) //unreachable
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_ret",
                        |mut caller: Caller<&mut Runtime<R>>, value_ptr: u32, value_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();
                            runtime.charge_host_function_call(
                                &host_function_costs.ret,
                                [value_ptr, value_size],
                            )?;
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            let error = runtime.ret(
                                function_context,
                                value_ptr,
                                value_size,
                                &mut scoped_instrumenter,
                            );
                            Result::<(), _>::Err(error.into())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_phase",
                        |mut caller: Caller<&mut Runtime<R>>, dest_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();
                            runtime.charge_host_function_call(
                                &host_function_costs.get_phase,
                                [dest_ptr],
                            )?;
                            runtime.get_phase(function_context, dest_ptr)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_is_valid_uref",
                        |mut caller: Caller<&mut Runtime<R>>, uref_ptr: u32, uref_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret =
                                runtime.is_valid_uref(function_context, uref_ptr, uref_size)?;
                            Ok(i32::from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_add_associated_key",
                        |mut caller: Caller<&mut Runtime<R>>,
                         account_hash_ptr: u32,
                         account_hash_size: u32,
                         weight: i32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.add_associated_key(
                                function_context,
                                account_hash_ptr,
                                account_hash_size as usize,
                                weight as u8,
                            )?;
                            Ok(ret)
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_remove_associated_key",
                        |mut caller: Caller<&mut Runtime<R>>,
                         account_hash_ptr: u32,
                         account_hash_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.remove_associated_key(
                                function_context,
                                account_hash_ptr,
                                account_hash_size as usize,
                            )?;
                            Ok(ret)
                            // Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_update_associated_key",
                        |mut caller: Caller<&mut Runtime<R>>,
                         account_hash_ptr: u32,
                         account_hash_size: u32,
                         weight: i32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.update_associated_key(
                                function_context,
                                account_hash_ptr,
                                account_hash_size as usize,
                                weight as u8,
                            )?;
                            Ok(ret)
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_set_action_threshold",
                        |mut caller: Caller<&mut Runtime<R>>,
                         permission_level: u32,
                         permission_threshold: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.set_action_threshold(
                                function_context,
                                permission_level,
                                permission_threshold as u8,
                            )?;
                            Ok(ret)
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_caller",
                        |mut caller: Caller<&mut Runtime<R>>, output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.get_caller(function_context, output_size_ptr)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_blocktime",
                        |mut caller: Caller<&mut Runtime<R>>, dest_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.get_blocktime(function_context, dest_ptr)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "gas",
                        |mut caller: Caller<&mut Runtime<R>>, param: u32| {
                            caller.data_mut().gas(Gas::new(U512::from(param)))?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_new_uref",
                        |mut caller: Caller<&mut Runtime<R>>, uref_ptr, value_ptr, value_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.casper_new_uref(
                                function_context,
                                uref_ptr,
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
                        "casper_create_purse",
                        |mut caller: Caller<&mut Runtime<R>>, dest_ptr: u32, dest_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_create_purse(
                                function_context,
                                dest_ptr,
                                dest_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_write",
                        |mut caller: Caller<&mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         value_ptr: u32,
                         value_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
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
                        |mut caller: Caller<&mut Runtime<R>>, dest_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.casper_get_main_purse(function_context, dest_ptr)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_named_arg_size",
                        |mut caller: Caller<&mut Runtime<R>>,
                         name_ptr: u32,
                         name_size: u32,
                         size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_get_named_arg_size(
                                function_context,
                                name_ptr,
                                name_size,
                                size_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_named_arg",
                        |mut caller: Caller<&mut Runtime<R>>,
                         name_ptr: u32,
                         name_size: u32,
                         dest_ptr: u32,
                         dest_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_get_named_arg(
                                function_context,
                                name_ptr,
                                name_size,
                                dest_ptr,
                                dest_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_transfer_to_account",
                        |mut caller: Caller<&mut Runtime<R>>,
                         key_ptr: u32,
                         key_size: u32,
                         amount_ptr: u32,
                         amount_size: u32,
                         id_ptr: u32,
                         id_size: u32,
                         result_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_transfer_to_account(
                                function_context,
                                key_ptr,
                                key_size,
                                amount_ptr,
                                amount_size,
                                id_ptr,
                                id_size,
                                result_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_has_key",
                        |mut caller: Caller<&mut Runtime<R>>, name_ptr: u32, name_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.has_key(function_context, name_ptr, name_size)?;
                            Ok(ret)
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_key",
                        |mut caller: Caller<&mut Runtime<R>>,
                         name_ptr: u32,
                         name_size: u32,
                         output_ptr: u32,
                         output_size: u32,
                         bytes_written: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.load_key(
                                function_context,
                                name_ptr,
                                name_size,
                                output_ptr,
                                output_size as usize,
                                bytes_written,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_put_key",
                        |mut caller: Caller<&mut Runtime<R>>,
                         name_ptr,
                         name_size,
                         key_ptr,
                         key_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.casper_put_key(
                                function_context,
                                name_ptr,
                                name_size,
                                key_ptr,
                                key_size,
                            )?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_remove_key",
                        |mut caller: Caller<&mut Runtime<R>>, name_ptr: u32, name_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.remove_key(function_context, name_ptr, name_size)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                #[cfg(feature = "test-support")]
                linker
                    .func_wrap(
                        "env",
                        "casper_print",
                        |mut caller: Caller<&mut Runtime<R>>, text_ptr: u32, text_size: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.casper_print(function_context, text_ptr, text_size)?;
                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_transfer_from_purse_to_purse",
                        |mut caller: Caller<&mut Runtime<R>>,
                         source_ptr,
                         source_size,
                         target_ptr,
                         target_size,
                         amount_ptr,
                         amount_size,
                         id_ptr,
                         id_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_transfer_from_purse_to_purse(
                                function_context,
                                source_ptr,
                                source_size,
                                target_ptr,
                                target_size,
                                amount_ptr,
                                amount_size,
                                id_ptr,
                                id_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_transfer_from_purse_to_account",
                        |mut caller: Caller<&mut Runtime<R>>,
                         source_ptr,
                         source_size,
                         key_ptr,
                         key_size,
                         amount_ptr,
                         amount_size,
                         id_ptr,
                         id_size,
                         result_ptr| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_transfer_from_purse_to_account(
                                function_context,
                                source_ptr,
                                source_size,
                                key_ptr,
                                key_size,
                                amount_ptr,
                                amount_size,
                                id_ptr,
                                id_size,
                                result_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_balance",
                        |mut caller: Caller<&mut Runtime<R>>,
                         ptr: u32,
                         ptr_size: u32,
                         output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.get_balance,
                                [ptr, ptr_size, output_size_ptr],
                            )?;
                            let ret = runtime.get_balance_host_buffer(
                                function_context,
                                ptr,
                                ptr_size as usize,
                                output_size_ptr,
                            )?;

                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_read_host_buffer",
                        |mut caller: Caller<&mut Runtime<R>>,
                         dest_ptr,
                         dest_size,
                         bytes_written_ptr| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.read_host_buffer,
                                [dest_ptr, dest_size, bytes_written_ptr],
                            )?;
                            // scoped_instrumenter.add_property("dest_size", dest_size);
                            let ret = runtime.read_host_buffer(
                                function_context,
                                dest_ptr,
                                dest_size as usize,
                                bytes_written_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_system_contract",
                        |mut caller: Caller<&mut Runtime<R>>,
                         system_contract_index,
                         dest_ptr,
                         dest_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.get_system_contract,
                                [system_contract_index, dest_ptr, dest_size],
                            )?;
                            let ret = runtime.get_system_contract(
                                function_context,
                                system_contract_index,
                                dest_ptr,
                                dest_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_load_named_keys",
                        |mut caller: Caller<&mut Runtime<R>>, total_keys_ptr, result_size_ptr| {
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::LoadNamedKeysFuncIndex);

                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.load_named_keys,
                                [total_keys_ptr, result_size_ptr],
                            )?;
                            let ret = runtime.load_named_keys(
                                function_context,
                                total_keys_ptr,
                                result_size_ptr,
                                &mut scoped_instrumenter,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_create_contract_package_at_hash",
                        |mut caller: Caller<&mut Runtime<R>>,
                         hash_dest_ptr,
                         access_dest_ptr,
                         is_locked: u32| {
                            let (mut function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.create_contract_package_at_hash,
                                [hash_dest_ptr, access_dest_ptr],
                            )?;
                            let package_status = ContractPackageStatus::new(is_locked != 0);
                            let (hash_addr, access_addr) =
                                runtime.create_contract_package_at_hash(package_status)?;

                            runtime.function_address(
                                &mut function_context,
                                hash_addr,
                                hash_dest_ptr,
                            )?;
                            runtime.function_address(
                                &mut function_context,
                                access_addr,
                                access_dest_ptr,
                            )?;

                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_create_contract_user_group",
                        |mut caller: Caller<&mut Runtime<R>>,
                         package_key_ptr: u32,
                         package_key_size: u32,
                         label_ptr: u32,
                         label_size: u32,
                         num_new_urefs: u32,
                         existing_urefs_ptr: u32,
                         existing_urefs_size: u32,
                         output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let ret = runtime.casper_create_contract_user_group(
                                function_context,
                                package_key_ptr,
                                package_key_size,
                                label_ptr,
                                label_size,
                                num_new_urefs,
                                existing_urefs_ptr,
                                existing_urefs_size,
                                output_size_ptr,
                            )?;

                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_provision_contract_user_group_uref",
                        |mut caller: Caller<&mut Runtime<R>>,
                         package_ptr,
                         package_size,
                         label_ptr,
                         label_size,
                         value_size_ptr| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.provision_contract_user_group_uref,
                                [
                                    package_ptr,
                                    package_size,
                                    label_ptr,
                                    label_size,
                                    value_size_ptr,
                                ],
                            )?;
                            let ret = runtime.provision_contract_user_group_uref(
                                function_context,
                                package_ptr,
                                package_size,
                                label_ptr,
                                label_size,
                                value_size_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();
                linker
                    .func_wrap(
                        "env",
                        "casper_remove_contract_user_group",
                        |mut caller: Caller<&mut Runtime<R>>,
                         package_key_ptr,
                         package_key_size,
                         label_ptr,
                         label_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let ret = runtime.casper_remove_contract_user_group(
                                function_context,
                                package_key_ptr,
                                package_key_size,
                                label_ptr,
                                label_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_remove_contract_user_group_urefs",
                        |mut caller: Caller<&mut Runtime<R>>,
                         package_ptr,
                         package_size,
                         label_ptr,
                         label_size,
                         urefs_ptr,
                         urefs_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.remove_contract_user_group_urefs,
                                [
                                    package_ptr,
                                    package_size,
                                    label_ptr,
                                    label_size,
                                    urefs_ptr,
                                    urefs_size,
                                ],
                            )?;
                            let ret = runtime.remove_contract_user_group_urefs(
                                function_context,
                                package_ptr,
                                package_size,
                                label_ptr,
                                label_size,
                                urefs_ptr,
                                urefs_size,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_call_versioned_contract",
                        |mut caller: Caller<&mut Runtime<R>>,
                         contract_package_hash_ptr,
                         contract_package_hash_size,
                         contract_version_ptr,
                         contract_package_size,
                         entry_point_name_ptr,
                         entry_point_name_size,
                         args_ptr,
                         args_size,
                         result_size_ptr| {
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            runtime.charge_host_function_call(
                                &host_function_costs.call_versioned_contract,
                                [
                                    contract_package_hash_ptr,
                                    contract_package_hash_size,
                                    contract_version_ptr,
                                    contract_package_size,
                                    entry_point_name_ptr,
                                    entry_point_name_size,
                                    args_ptr,
                                    args_size,
                                    result_size_ptr,
                                ],
                            )?;

                            // TODO: Move

                            let contract_package_hash: ContractPackageHash = {
                                let contract_package_hash_bytes = function_context
                                    .memory_read(
                                        contract_package_hash_ptr,
                                        contract_package_hash_size as usize,
                                    )
                                    .map_err(|e| execution::Error::Interpreter(e.to_string()))?;
                                bytesrepr::deserialize(contract_package_hash_bytes)
                                    .map_err(execution::Error::BytesRepr)?
                            };
                            let contract_version: Option<ContractVersion> = {
                                let contract_version_bytes = function_context
                                    .memory_read(
                                        contract_version_ptr,
                                        contract_package_size as usize,
                                    )
                                    .map_err(|e| execution::Error::Interpreter(e.to_string()))?;
                                bytesrepr::deserialize(contract_version_bytes)
                                    .map_err(execution::Error::BytesRepr)?
                            };
                            let entry_point_name: String = {
                                let entry_point_name_bytes = function_context
                                    .memory_read(
                                        entry_point_name_ptr,
                                        entry_point_name_size as usize,
                                    )
                                    .map_err(|e| execution::Error::Interpreter(e.to_string()))?;
                                bytesrepr::deserialize(entry_point_name_bytes)
                                    .map_err(execution::Error::BytesRepr)?
                            };
                            let args_bytes: Vec<u8> = function_context
                                .memory_read(args_ptr, args_size as usize)
                                .map_err(|e| execution::Error::Interpreter(e.to_string()))?;

                            let ret = runtime.call_versioned_contract_host_buffer(
                                function_context,
                                contract_package_hash,
                                contract_version,
                                entry_point_name,
                                args_bytes,
                                result_size_ptr,
                                &mut scoped_instrumenter,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_add_contract_version",
                        |mut caller: Caller<&mut Runtime<R>>,
                         contract_package_hash_ptr,
                         contract_package_hash_size,
                         version_ptr,
                         entry_points_ptr,
                         entry_points_size,
                         named_keys_ptr,
                         named_keys_size,
                         output_ptr,
                         output_size,
                         bytes_written_ptr| {
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            let ret = runtime.casper_add_contract_version(
                                function_context,
                                contract_package_hash_ptr,
                                contract_package_hash_size,
                                version_ptr,
                                entry_points_ptr,
                                entry_points_size,
                                named_keys_ptr,
                                named_keys_size,
                                output_ptr,
                                output_size,
                                bytes_written_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_call_contract",
                        |mut caller: Caller<&mut Runtime<R>>,
                         contract_hash_ptr,
                         contract_hash_size,
                         entry_point_name_ptr,
                         entry_point_name_size,
                         args_ptr,
                         args_size,
                         result_size_ptr| {
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);

                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();

                            let ret = runtime.casper_call_contract(
                                function_context,
                                contract_hash_ptr,
                                contract_hash_size,
                                entry_point_name_ptr,
                                entry_point_name_size,
                                args_ptr,
                                args_size,
                                result_size_ptr,
                                &mut scoped_instrumenter,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_load_call_stack",
                        |mut caller: Caller<&mut Runtime<R>>,
                         call_stack_len_ptr,
                         result_size_ptr| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.charge_host_function_call(
                                &host_function_costs::HostFunction::fixed(10_000),
                                [call_stack_len_ptr, result_size_ptr],
                            )?;
                            let ret = runtime.load_call_stack(
                                function_context,
                                call_stack_len_ptr,
                                result_size_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_new_dictionary",
                        |mut caller: Caller<&mut Runtime<R>>, output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            runtime.charge_host_function_call(
                                &host_function_costs::DEFAULT_HOST_FUNCTION_NEW_DICTIONARY,
                                [output_size_ptr],
                            )?;
                            let ret = runtime.new_dictionary(function_context, output_size_ptr)?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_dictionary_get",
                        |mut caller: Caller<&mut Runtime<R>>,
                         uref_ptr: u32,
                         uref_size: u32,
                         key_bytes_ptr: u32,
                         key_bytes_size: u32,
                         output_size_ptr: u32| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();
                            runtime.charge_host_function_call(
                                &host_function_costs.dictionary_get,
                                [key_bytes_ptr, key_bytes_size, output_size_ptr],
                            )?;
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            let ret = runtime.dictionary_get(
                                function_context,
                                uref_ptr,
                                uref_size,
                                key_bytes_ptr,
                                key_bytes_size,
                                output_size_ptr,
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_dictionary_put",
                        |mut caller: Caller<&mut Runtime<R>>,
                         uref_ptr,
                         uref_size,
                         key_bytes_ptr,
                         key_bytes_size,
                         value_ptr,
                         value_ptr_size| {
                            let (function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            scoped_instrumenter.add_property("key_bytes_size", key_bytes_size);
                            scoped_instrumenter.add_property("value_size", value_ptr_size);
                            let ret = runtime.dictionary_put(
                                function_context,
                                uref_ptr,
                                uref_size,
                                key_bytes_ptr,
                                key_bytes_size,
                                value_ptr,
                                value_ptr_size,
                            )?;
                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();
                            runtime.charge_host_function_call(
                                &host_function_costs.dictionary_put,
                                [key_bytes_ptr, key_bytes_size, value_ptr, value_ptr_size],
                            )?;
                            Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_blake2b",
                        |mut caller: Caller<&mut Runtime<R>>,
                         in_ptr: u32,
                         in_size: u32,
                         out_ptr: u32,
                         out_size: u32| {
                            let (mut function_context, runtime) =
                                caller_adapter_and_runtime(&mut caller);
                            let host_function_costs =
                                runtime.config().wasm_config().take_host_function_costs();
                            runtime.charge_host_function_call(
                                &host_function_costs.blake2b,
                                [in_ptr, in_size, out_ptr, out_size],
                            )?;
                            let mut scoped_instrumenter =
                                ScopedInstrumenter::new(FunctionIndex::CallVersionedContract);
                            scoped_instrumenter.add_property("in_size", in_size.to_string());
                            scoped_instrumenter.add_property("out_size", out_size.to_string());
                            let digest = account::blake2b(
                                function_context
                                    .memory_read(in_ptr, in_size as usize)
                                    .map_err(|e| execution::Error::Interpreter(e.into()))?,
                            );
                            if digest.len() != out_size as usize {
                                let err_value =
                                    u32::from(api_error::ApiError::BufferTooSmall) as i32;
                                return Ok(err_value);
                            }
                            function_context
                                .memory_write(out_ptr, &digest)
                                .map_err(|e| execution::Error::Interpreter(e.into()))?;
                            Ok(0_i32)
                        },
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

// TODO: There are some issues with multithreaded test runner and how we set up the cache. We should
// figure out better way to deal with this initialization.
static GLOBAL_CACHE_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn new_compiled_engine(wasm_config: &WasmConfig) -> WasmtimeEngine {
    let mut config = wasmtime::Config::new();

    // This should solve nondeterministic issue with access to the directory when running EE in
    // non-compiled mode
    {
        let _global_cache_guard = GLOBAL_CACHE_MUTEX.lock().unwrap();
        setup_wasmtime_caching(&Path::new("/tmp/wasmtime_test"), &mut config)
            .expect("should setup wasmtime cache path");
    }

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

        if memory_section(&module).is_none() {
            // `pwasm_utils::externalize_mem` expects a memory section to exist in the
            // module, and panics otherwise.
            return Err(PreprocessingError::MissingMemorySection);
        }

        let module = pwasm_utils::externalize_mem(module, None, self.wasm_config.max_memory);

        let module = stack_height::inject_limiter(module, self.wasm_config.max_stack_height)
            .map_err(|_| PreprocessingError::StackLimiter)?;

        match self.execution_mode {
            ExecutionMode::Interpreted => Ok(module.into()),
            ExecutionMode::Compiled => {
                // TODO: Gas injected module is used here but we might want to use `module` instead
                // with other preprocessing done.
                let preprocessed_wasm_bytes =
                    parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes");

                // aot compile
                let precompiled_bytes = self.precompile(&preprocessed_wasm_bytes).unwrap();
                // let wasmtime_module =
                //     wasmtime::Module::new(&self.compiled_engine, &preprocessed_wasm_bytes)
                //         .expect("should process");
                Ok(Module::Compiled {
                    wasmi_module: module,
                    compiled_artifact: precompiled_bytes.to_owned(),
                })

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
        let parity_module = deserialize_interpreted(wasm_bytes)?;

        let module = match self.execution_mode {
            ExecutionMode::Interpreted => Module::Interpreted(parity_module),
            ExecutionMode::Compiled => {
                // aot compile
                let precompiled_bytes = self.precompile(wasm_bytes).unwrap();
                // self.compiled_engine.precompile_module(&wasm_bytes).expect("should preprocess");
                // let module = wasmtime::Module::new(&self.compiled_engine, wasm_bytes).unwrap();
                Module::Compiled {
                    wasmi_module: parity_module,
                    compiled_artifact: precompiled_bytes.to_owned(),
                }
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
            Module::Noop => Ok(Instance::Noop),
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
            Module::Compiled {
                wasmi_module,
                compiled_artifact,
            } => {
                // aot compile
                // let precompiled_bytes =
                // self.compiled_engine.precompile_module(&preprocessed_wasm_bytes).expect("should
                // preprocess"); Ok(Module::Compiled(precompiled_bytes))

                // todo!("compiled mode")
                // let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine(), ());
                // let instance = wasmtime::Instance::new(&mut store, &compiled_module,
                // &[]).expect("should create compiled module");

                let compiled_module = self.deserialize_compiled(&compiled_artifact)?;
                Ok(Instance::Compiled {
                    module: wasmi_module,
                    compiled_module,
                })
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
