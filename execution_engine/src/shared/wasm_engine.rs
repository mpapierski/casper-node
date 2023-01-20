//! Preprocessing of Wasm modules.
use bytes::Bytes;
use casper_types::{
    account::{self, AccountHash},
    api_error, blake2b,
    bytesrepr::{self, FromBytes, ToBytes},
    contracts::ContractPackageStatus,
    AccessRights, ApiError, CLValue, ContractPackageHash, ContractVersion, Gas, Key,
    ProtocolVersion, StoredValue, URef, U512,
};
use itertools::Itertools;
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use once_cell::sync::Lazy;
use parity_wasm::elements::{
    self, External, Instruction, Internal, MemorySection, Section, TableType, Type,
};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    borrow::{BorrowMut, Cow},
    cell::{Cell, RefCell},
    collections::{hash_map::Entry, HashMap},
    error::Error,
    fmt::{self, Display, Formatter},
    fs::{self, File, OpenOptions},
    io::Write,
    ops::{Deref, DerefMut},
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex, Once, RwLock},
    time::{Duration, Instant},
};
use thiserror::Error;
use tracing::Subscriber;
use wasmer::{
    imports, AsStoreMut, AsStoreRef, Cranelift, FunctionEnv, FunctionEnvMut, TypedFunction,
};
use wasmer_compiler_singlepass::Singlepass;

pub(crate) mod host_interface;
mod macros;
mod wasmer_backend;
mod wasmi_backend;
mod wasmtime_backend;

const DEFAULT_GAS_MODULE_NAME: &str = "env";
/// Name of the internal gas function injected by [`pwasm_utils::inject_gas_counter`].
const INTERNAL_GAS_FUNCTION_NAME: &str = "gas";

/// We only allow maximum of 4k function pointers in a table section.
pub const DEFAULT_MAX_TABLE_SIZE: u32 = 4096;
/// Maximum number of elements that can appear as immediate value to the br_table instruction.
pub const DEFAULT_BR_TABLE_MAX_SIZE: u32 = 256;
/// Maximum number of global a module is allowed to declare.
pub const DEFAULT_MAX_GLOBALS: u32 = 256;
/// Maximum number of parameters a function can have.
pub const DEFAULT_MAX_PARAMETER_COUNT: u32 = 256;

pub use parity_wasm::elements::Module as WasmiModule;
use wasmi::{ImportsBuilder, MemoryRef, ModuleInstance, ModuleRef};
use wasmtime::{
    AsContext, AsContextMut, Caller, Extern, ExternType, InstanceAllocationStrategy, Memory,
    MemoryType, StoreContextMut, Trap,
};

use crate::{
    core::{
        execution,
        runtime::{Runtime, RuntimeStack},
    },
    shared::host_function_costs,
    storage::global_state::StateReader,
};

use self::{
    wasmer_backend::{WasmerAdapter, WasmerEnv},
    wasmi_backend::{WasmiExternals, WasmiResolver},
    wasmtime_backend::WasmtimeEnv,
};

use super::{
    newtypes::{CorrelationId, Property},
    opcode_costs::OpcodeCosts,
    wasm_config::WasmConfig,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CraneliftOptLevel {
    None,
    Speed,
    SpeedAndSize,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WasmerBackend {
    Singlepass,
    Cranelift { optimize: CraneliftOptLevel },
}

/// Mode of execution for smart contracts.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Runs Wasm modules in interpreted mode.
    Interpreted,
    /// Runs Wasm modules in a compiled mode.
    Compiled { cache_artifacts: bool },
    /// Runs Wasm modules in a JIT mode.
    JustInTime,
    /// Runs Wasm modules in a compiled mode via fast ahead of time compiler.
    ///
    /// NOTE: Currently cache is implemented as inmem hash table for testing purposes
    Wasmer {
        backend: WasmerBackend,
        cache_artifacts: bool,
    },
}

impl ExecutionMode {
    /// Returns `true` if the execution mode is [`Singlepass`].
    ///
    /// [`Singlepass`]: ExecutionMode::Singlepass
    #[must_use]
    pub fn is_singlepass(&self) -> bool {
        matches!(self, Self::Wasmer { .. })
    }

    pub fn is_using_cache(&self) -> bool {
        match self {
            ExecutionMode::Interpreted => false,
            ExecutionMode::Compiled { cache_artifacts } => *cache_artifacts,
            ExecutionMode::JustInTime => false,
            ExecutionMode::Wasmer {
                cache_artifacts, ..
            } => *cache_artifacts,
        }
    }
}

impl ToBytes for ExecutionMode {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        match self {
            ExecutionMode::Interpreted => 1u32.to_bytes(),
            ExecutionMode::Compiled { cache_artifacts } => (2u32, *cache_artifacts).to_bytes(),
            ExecutionMode::JustInTime => 3u32.to_bytes(),
            ExecutionMode::Wasmer {
                cache_artifacts, ..
            } => (4u32, *cache_artifacts).to_bytes(),
        }
    }

    fn serialized_length(&self) -> usize {
        match self {
            ExecutionMode::Interpreted => 1u32.serialized_length(),
            ExecutionMode::Compiled { cache_artifacts } => {
                (2u32, *cache_artifacts).serialized_length()
            }
            ExecutionMode::JustInTime => 3u32.serialized_length(),
            ExecutionMode::Wasmer {
                backend,
                cache_artifacts,
                ..
            } => (4u32, *cache_artifacts).serialized_length(),
        }
    }
}

impl FromBytes for ExecutionMode {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (execution_mode, rem) = u32::from_bytes(bytes)?;
        if execution_mode == 1 {
            Ok((ExecutionMode::Interpreted, rem))
        } else if execution_mode == 2 {
            let (flag, rem) = bool::from_bytes(rem)?;
            Ok((
                ExecutionMode::Compiled {
                    cache_artifacts: flag,
                },
                rem,
            ))
        } else if execution_mode == 3 {
            Ok((ExecutionMode::JustInTime, rem))
        } else if execution_mode == 4 {
            // let (backend, rem) = u32::from_bytes(rem)?;
            // let backend = WasmerBackend::from_u32(backend).ok_or(bytesrepr::Error::Formatting)?;
            let (flag, rem) = bool::from_bytes(rem)?;
            Ok((ExecutionMode::Interpreted, rem))
        } else {
            Err(bytesrepr::Error::Formatting)
        }
    }
}

impl Distribution<ExecutionMode> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> ExecutionMode {
        match rng.gen::<bool>() {
            false => ExecutionMode::Interpreted,
            true => ExecutionMode::Compiled {
                cache_artifacts: rng.gen(),
            },
        }
    }
}

fn deserialize_interpreted(wasm_bytes: &[u8]) -> Result<WasmiModule, PreprocessingError> {
    parity_wasm::elements::deserialize_buffer(wasm_bytes).map_err(PreprocessingError::from)
}

/// Statically dispatched Wasm module wrapper for different implementations
/// NOTE: Probably at this stage it can hold raw wasm bytes without being an enum, and the decision
/// can be made later
pub enum Module {
    /// TODO: We might not need it anymore. We use "do nothing" bytes for replacing wasm modules
    Noop,
    Interpreted {
        original_bytes: Bytes,
        wasmi_module: WasmiModule,
        timestamp: Instant,
    },
    Compiled {
        original_bytes: Bytes,
        /// Used to carry on original Wasm module for easy further processing.
        wasmi_module: WasmiModule,
        precompile_time: Option<Duration>,
        /// Ahead of time compiled artifact.
        // compiled_artifact: Bytes,
        wasmtime_module: wasmtime::Module,
    },
    Jitted {
        original_bytes: Bytes,
        wasmi_module: WasmiModule,
        precompile_time: Option<Duration>,
        module: wasmtime::Module,
    },
    Singlepass {
        original_bytes: Bytes,
        wasmi_module: WasmiModule,
        wasmer_module: wasmer::Module,
        store: wasmer::Store,
        timestamp: Instant,
    },
}

impl fmt::Debug for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noop => f.debug_tuple("Noop").finish(),
            Self::Interpreted { wasmi_module, .. } => {
                f.debug_tuple("Interpreted").field(wasmi_module).finish()
            }
            Self::Compiled { .. } => f.debug_tuple("Compiled").finish(),
            Self::Jitted { .. } => f.debug_tuple("Jitted").finish(),
            Module::Singlepass { .. } => f.debug_tuple("Singlepass").finish(),
        }
    }
}

impl Module {
    pub fn try_into_interpreted(self) -> Result<WasmiModule, Self> {
        if let Self::Interpreted { wasmi_module, .. } = self {
            Ok(wasmi_module)
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
            Module::Interpreted {
                original_bytes,
                wasmi_module,
                ..
            } => wasmi_module,
            Module::Compiled { wasmi_module, .. } => wasmi_module,
            Module::Jitted { wasmi_module, .. } => wasmi_module,
            Module::Singlepass { wasmi_module, .. } => wasmi_module,
        }
    }

    pub fn get_wasmi_module(&self) -> WasmiModule {
        let wasmi_module_ref = match self {
            Module::Noop => unreachable!("noop is unused"),
            Module::Interpreted {
                original_bytes,
                wasmi_module,
                ..
            } => wasmi_module,
            Module::Compiled {
                original_bytes,
                wasmi_module,
                precompile_time,

                wasmtime_module,
            } => wasmi_module,
            Module::Jitted {
                original_bytes,
                wasmi_module,
                precompile_time,
                module,
            } => wasmi_module,
            Module::Singlepass { wasmi_module, .. } => wasmi_module,
        };
        wasmi_module_ref.clone()
    }
}

/// Common error type adapter for all Wasm engines supported.
#[derive(Error, Clone, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    WasmiError(Arc<wasmi::Error>),
    #[error(transparent)]
    WasmtimeError(#[from] wasmtime::Trap),
    #[error(transparent)]
    WasmerError(#[from] wasmer::RuntimeError),
    #[error(transparent)]
    MemoryAccessError(#[from] wasmer::MemoryAccessError),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Instantiation(Arc<wasmer::InstantiationError>),
}

impl From<wasmer::InstantiationError> for RuntimeError {
    fn from(v: wasmer::InstantiationError) -> Self {
        Self::Instantiation(Arc::new(v))
    }
}

impl From<wasmi::Error> for RuntimeError {
    fn from(error: wasmi::Error) -> Self {
        RuntimeError::WasmiError(Arc::new(error))
    }
}

impl From<String> for RuntimeError {
    fn from(string: String) -> Self {
        Self::Other(string)
    }
}

impl RuntimeError {
    /// Extracts the executor error from a runtime Wasm trap.
    pub fn into_execution_error(self) -> Result<execution::Error, Self> {
        match &self {
            RuntimeError::WasmiError(wasmi_error) => {
                // todo!()
                match wasmi_error.as_host_error().and_then(|host_error| {
                    host_error.downcast_ref::<wasmi_backend::IndirectHostError<execution::Error>>()
                }) {
                    Some(indirect) => Ok(indirect.source.clone()),
                    None => Err(self),
                }
                // Ok(error)
            }
            RuntimeError::WasmtimeError(wasmtime_trap) => {
                match wasmtime_trap
                    .source()
                    .and_then(|src| src.downcast_ref::<execution::Error>())
                {
                    Some(execution_error) => Ok(execution_error.clone()),
                    None => Err(self),
                }
            }
            RuntimeError::WasmerError(wasmer_runtime_error) => {
                match wasmer_runtime_error
                    // .clone()
                    .source()
                    .and_then(|src| src.downcast_ref::<execution::Error>())
                {
                    Some(execution_error) => Ok(execution_error.clone()),
                    None => Err(self),
                }
            }
            _ => Err(self),
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
pub enum Instance<R>
where
    R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
    R::Error: Into<execution::Error>,
{
    // TODO: We might not need it. Actually trying to execute a noop is a programming error.
    Noop,
    Interpreted {
        original_bytes: Bytes,
        module: ModuleRef,
        memory: MemoryRef,
        runtime: Runtime<R>,
        timestamp: Instant,
    },
    // NOTE: Instance should contain wasmtime::Instance instead but we need to hold Store that has
    // a lifetime and a generic R
    Compiled {
        /// For metrics
        original_bytes: Bytes,
        precompile_time: Option<Duration>,
        /// Raw Wasmi module used only for further processing.
        module: WasmiModule,
        /// This is compiled module used for execution.
        compiled_module: wasmtime::Module,
        runtime: Runtime<R>,
    },
    Singlepass {
        original_bytes: Bytes,
        module: WasmiModule,
        /// There is no separate "compile" step that turns a wasm bytes into a wasmer module
        wasmer_module: wasmer::Module,
        runtime: Runtime<R>,
        store: wasmer::Store,
        timestamp: Instant,
    },
}

unsafe impl<R> Send for Instance<R>
where
    R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
    R::Error: Into<execution::Error>,
{
}
// unsafe impl<R> Sync for Instance<R> where R: Sync {}

impl<R> fmt::Debug for Instance<R>
where
    R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
    R::Error: Into<execution::Error>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Noop => f.debug_tuple("Noop").finish(),
            Self::Interpreted { module, memory, .. } => f
                .debug_tuple("Interpreted")
                .field(module)
                .field(memory)
                .finish(),
            Self::Compiled { .. } => f.debug_tuple("Compiled").finish(),
            Self::Singlepass { .. } => f.debug_tuple("Singlepass").finish(),
        }
    }
}

// #[derive(Debug, Clone)]
// pub struct InstanceRef<R>(Rc<Instance>);

// impl From<Instance> for InstanceRef {
//     fn from(instance: Instance) -> Self {
//         InstanceRef(Rc::new(instance))
//     }
// }

pub trait FunctionContext {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError>;
    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), RuntimeError>;
}

impl<R> Instance<R>
where
    R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
    R::Error: Into<execution::Error>,
{
    pub fn interpreted_memory(&self) -> MemoryRef {
        match self {
            Instance::Noop => unimplemented!("Unable to get interpreted memory from noop module"),
            Instance::Interpreted { memory, .. } => memory.clone(),
            Instance::Compiled { .. } => unreachable!("available only from wasmi externals"),
            Instance::Singlepass { .. } => unreachable!("available only from wasmi externals"),
        }
    }
    /// Invokes exported function
    pub fn invoke_export(
        self,
        correlation_id: CorrelationId,
        wasm_engine: &WasmEngine,
        func_name: &str,
        args: Vec<RuntimeValue>,
    ) -> Result<Option<RuntimeValue>, RuntimeError> {
        match self {
            Instance::Noop => {
                unimplemented!("Unable to execute export {} on noop instance", func_name)
            }
            Instance::Interpreted {
                original_bytes,
                module,
                memory,
                mut runtime,
                timestamp,
            } => {
                let wasmi_args: Vec<wasmi::RuntimeValue> =
                    args.into_iter().map(|value| value.into()).collect();

                let mut wasmi_externals = WasmiExternals {
                    host: &mut runtime,
                    memory: memory.clone(),
                };

                let preprocess_dur = timestamp.elapsed();
                // let exec_start = Instant::now();

                let wasmi_result =
                    module.invoke_export(func_name, wasmi_args.as_slice(), &mut wasmi_externals);

                let invoke_dur = timestamp.elapsed();

                let wasmi_result = wasmi_result?;

                // wasmi's runtime value into our runtime value
                let result = wasmi_result.map(RuntimeValue::from);

                correlation_id.record_property(Property::WasmVM {
                    original_bytes: original_bytes.clone(),
                    preprocess_duration: preprocess_dur,
                    invoke_duration: invoke_dur,
                });

                Ok(result)
            }
            Instance::Compiled {
                original_bytes,
                module,
                mut compiled_module,
                precompile_time,
                runtime,
            } => {
                // record_performance_log(&original_bytes,
                // LogProperty::EntryPoint(func_name.to_string()));

                let mut wasmtime_env = WasmtimeEnv {
                    host: runtime,
                    wasmtime_memory: None,
                };

                let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine, wasmtime_env);

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

                // debug_assert_eq!(
                //     memory_type.maximum(),
                //     Some(wasm_engine.wasm_config().max_memory as u64)
                // );

                let memory = wasmtime::Memory::new(&mut store, memory_type).unwrap();
                store.data_mut().wasmtime_memory = Some(memory);
                let mut linker =
                    wasmtime_backend::make_linker_object("env", &wasm_engine.compiled_engine); //wasmtime::Linker::new(&wasm_engine.compiled_engine);
                                                                                               // let mut linker = wasmtime_backend::make_linker_object("env",  let mut linker =
                                                                                               // wasmtime::Linker::new(&wasm_engine.compiled_engine););
                linker
                    .define("env", "memory", wasmtime::Extern::from(memory))
                    .unwrap();

                // linker
                //     .func_wrap(
                //         "env",
                //         "casper_read_value",
                //         |mut caller: Caller<Runtime<R>>,
                //          key_ptr: u32,
                //          key_size: u32,
                //          output_size_ptr: u32| {
                //             let (function_context, runtime) =
                //                 caller_adapter_and_runtime(&mut caller);
                //             let ret = runtime.read(
                //                 function_context,
                //                 key_ptr,
                //                 key_size,
                //                 output_size_ptr,
                //             )?;
                //             Ok(api_error::i32_from(ret))
                //         },
                //     )
                //     .unwrap();

                let start = Instant::now();

                let instance = linker
                    .instantiate(&mut store, &compiled_module)
                    .map_err(|error| RuntimeError::Other(error.to_string()))?;

                let exported_func = instance
                    .get_typed_func::<(), (), _>(&mut store, func_name)
                    .expect("should get typed func");

                let ret = exported_func
                    .call(&mut store, ())
                    .map_err(RuntimeError::from);

                let stop = start.elapsed();

                ret?;

                Ok(Some(RuntimeValue::I64(0)))
            }
            Instance::Singlepass {
                original_bytes,
                module,
                wasmer_module,
                runtime,
                store: mut wasmer_store,
                timestamp,
            } => {
                let preprocess_dur = timestamp.elapsed();

                let memory_pages = wasm_engine.wasm_config().max_memory;

                let imports = wasmer_module.imports().into_iter().collect::<Vec<_>>();
                let import_type = imports.iter().find_map(|import_type| {
                    if (import_type.module(), import_type.name()) == ("env", "memory") {
                        import_type.ty().memory().cloned()
                    } else {
                        None
                    }
                });

                // let memory_import = import_type;

                let imported_memory = if let Some(imported_memory) = import_type {
                    // dbg!(&imported_memory);

                    let memory = wasmer::Memory::new(
                        &mut wasmer_store.as_store_mut(),
                        imported_memory.clone(),
                    )
                    .expect("should create wasmer memory");
                    Some(memory)
                } else {
                    None
                };

                // let memory_obj = memory;
                // // import_object.mem

                let wasmer_env = WasmerEnv {
                    host: runtime,
                    memory: imported_memory.clone(),
                };

                let mut function_env =
                    FunctionEnv::new(&mut wasmer_store.as_store_mut(), wasmer_env);

                let mut import_object =
                    wasmer_backend::make_wasmer_imports("env", &mut wasmer_store, &function_env);

                if let Some(imported_memory) = &imported_memory {
                    import_object.define(
                        "env",
                        "memory",
                        wasmer::Extern::from(imported_memory.clone()),
                    );
                }

                let instance = wasmer::Instance::new(
                    &mut wasmer_store.as_store_mut(),
                    &wasmer_module,
                    &import_object,
                )?;

                // function_env.as_mut(&mut wasmer_store.as_store_mut(), )
                // instance.
                match instance.exports.get_memory("memory") {
                    Ok(exported_memory) => {
                        // dbg!(&exported_memory);
                        function_env.as_mut(&mut wasmer_store.as_store_mut()).memory =
                            Some(exported_memory.clone());
                    }
                    Err(error) => {
                        if imported_memory.is_none() {
                            panic!("Instance does not import/export memory which we don't currently allow {:?}", error);
                        }
                    }
                }

                let call_func: TypedFunction<(), ()> = instance
                    .exports
                    .get_typed_function(&wasmer_store.as_store_ref(), func_name)
                    .expect("should get wasmer call func");

                call_func.call(&mut wasmer_store.as_store_mut())?;

                let invoke_dur = timestamp.elapsed();

                correlation_id.record_property(Property::WasmVM {
                    original_bytes: original_bytes.clone(),
                    preprocess_duration: preprocess_dur,
                    invoke_duration: invoke_dur,
                });

                Ok(Some(RuntimeValue::I64(0)))
            }
        }
    }
}

/// An error emitted by the Wasm preprocessor.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum WasmValidationError {
    /// Initial table size outside allowed bounds.
    #[error("initial table size of {actual} exceeds allowed limit of {max}")]
    InitialTableSizeExceeded {
        /// Allowed maximum table size.
        max: u32,
        /// Actual initial table size specified in the Wasm.
        actual: u32,
    },
    /// Maximum table size outside allowed bounds.
    #[error("maximum table size of {actual} exceeds allowed limit of {max}")]
    MaxTableSizeExceeded {
        /// Allowed maximum table size.
        max: u32,
        /// Actual max table size specified in the Wasm.
        actual: u32,
    },
    /// Number of the tables in a Wasm must be at most one.
    #[error("the number of tables must be at most one")]
    MoreThanOneTable,
    /// Length of a br_table exceeded the maximum allowed size.
    #[error("maximum br_table size of {actual} exceeds allowed limit of {max}")]
    BrTableSizeExceeded {
        /// Maximum allowed br_table length.
        max: u32,
        /// Actual size of a br_table in the code.
        actual: usize,
    },
    /// Declared number of globals exceeds allowed limit.
    #[error("declared number of globals ({actual}) exceeds allowed limit of {max}")]
    TooManyGlobals {
        /// Maximum allowed globals.
        max: u32,
        /// Actual number of globals declared in the Wasm.
        actual: usize,
    },
    /// Module declares a function type with too many parameters.
    #[error("use of a function type with too many parameters (limit of {max} but function declares {actual})")]
    TooManyParameters {
        /// Maximum allowed parameters.
        max: u32,
        /// Actual number of parameters a function has in the Wasm.
        actual: usize,
    },
    /// Module tries to import a function that the host does not provide.
    #[error("module imports a non-existent function")]
    MissingHostFunction,
    /// Opcode for a global access refers to a non-existing global
    #[error("opcode for a global access refers to non-existing global index {index}")]
    IncorrectGlobalOperation {
        /// Provided index.
        index: u32,
    },
    /// Missing function index.
    #[error("missing function index {index}")]
    MissingFunctionIndex {
        /// Provided index.
        index: u32,
    },
    /// Missing function type.
    #[error("missing type index {index}")]
    MissingFunctionType {
        /// Provided index.
        index: u32,
    },
    /// Unsupported WASM start section.
    #[error("Unsupported WASM start")]
    UnsupportedWasmStart,
}

/// An error emitted by the Wasm preprocessor.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
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
    /// The module is missing.
    #[error("Missing module")]
    MissingModule,
    /// Unable to validate wasm bytes.
    #[error("Wasm validation error: {0}")]
    WasmValidation(#[from] WasmValidationError),
    /// Wasmtime was unable to precompile a module
    #[error("Precompile error: {0}")]
    Precompile(String),
    /// Wasmer was unable to compile a module
    #[error(transparent)]
    Compile(Arc<wasmer::CompileError>),
    #[error(transparent)]
    DeserializeModule(Arc<wasmer::DeserializeError>),
    #[error(transparent)]
    SerializeModule(Arc<wasmer::SerializeError>),
}

impl From<wasmer::CompileError> for PreprocessingError {
    fn from(v: wasmer::CompileError) -> Self {
        Self::Compile(Arc::new(v))
    }
}

impl From<wasmer::DeserializeError> for PreprocessingError {
    fn from(v: wasmer::DeserializeError) -> Self {
        Self::DeserializeModule(Arc::new(v))
    }
}

impl From<wasmer::SerializeError> for PreprocessingError {
    fn from(v: wasmer::SerializeError) -> Self {
        Self::SerializeModule(Arc::new(v))
    }
}

impl From<elements::Error> for PreprocessingError {
    fn from(error: elements::Error) -> Self {
        PreprocessingError::Deserialize(error.to_string())
    }
}

/// Ensures that all the references to functions and global variables in the wasm bytecode are
/// properly declared.
///
/// This validates that:
///
/// - Start function points to a function declared in the Wasm bytecode
/// - All exported functions are pointing to functions declared in the Wasm bytecode
/// - `call` instructions reference a function declared in the Wasm bytecode.
/// - `global.set`, `global.get` instructions are referencing an existing global declared in the
///   Wasm bytecode.
/// - All members of the "elem" section point at functions declared in the Wasm bytecode.
fn ensure_valid_access(module: &WasmiModule) -> Result<(), WasmValidationError> {
    let function_types_count = module
        .type_section()
        .map(|ts| ts.types().len())
        .unwrap_or_default();

    let mut function_count = 0_u32;
    if let Some(import_section) = module.import_section() {
        for import_entry in import_section.entries() {
            if let External::Function(function_type_index) = import_entry.external() {
                if (*function_type_index as usize) < function_types_count {
                    function_count = function_count.saturating_add(1);
                } else {
                    return Err(WasmValidationError::MissingFunctionType {
                        index: *function_type_index,
                    });
                }
            }
        }
    }
    if let Some(function_section) = module.function_section() {
        for function_entry in function_section.entries() {
            let function_type_index = function_entry.type_ref();
            if (function_type_index as usize) < function_types_count {
                function_count = function_count.saturating_add(1);
            } else {
                return Err(WasmValidationError::MissingFunctionType {
                    index: function_type_index,
                });
            }
        }
    }

    if let Some(function_index) = module.start_section() {
        ensure_valid_function_index(function_index, function_count)?;
    }
    if let Some(export_section) = module.export_section() {
        for export_entry in export_section.entries() {
            if let Internal::Function(function_index) = export_entry.internal() {
                ensure_valid_function_index(*function_index, function_count)?;
            }
        }
    }

    if let Some(code_section) = module.code_section() {
        let global_len = module
            .global_section()
            .map(|global_section| global_section.entries().len())
            .unwrap_or(0);

        for instr in code_section
            .bodies()
            .iter()
            .flat_map(|body| body.code().elements())
        {
            match instr {
                Instruction::Call(idx) => {
                    ensure_valid_function_index(*idx, function_count)?;
                }
                Instruction::GetGlobal(idx) | Instruction::SetGlobal(idx)
                    if *idx as usize >= global_len =>
                {
                    return Err(WasmValidationError::IncorrectGlobalOperation { index: *idx });
                }
                _ => {}
            }
        }
    }

    if let Some(element_section) = module.elements_section() {
        for element_segment in element_section.entries() {
            for idx in element_segment.members() {
                ensure_valid_function_index(*idx, function_count)?;
            }
        }
    }

    Ok(())
}

fn ensure_valid_function_index(index: u32, function_count: u32) -> Result<(), WasmValidationError> {
    if index >= function_count {
        return Err(WasmValidationError::MissingFunctionIndex { index });
    }
    Ok(())
}

/// Checks if given wasm module contains a memory section.
fn memory_section(module: &WasmiModule) -> Option<&MemorySection> {
    for section in module.sections() {
        if let Section::Memory(section) = section {
            return Some(section);
        }
    }
    None
}

/// Ensures (table) section has at most one table entry, and initial, and maximum values are
/// normalized.
///
/// If a maximum value is not specified it will be defaulted to 4k to prevent OOM.
fn ensure_table_size_limit(
    mut module: WasmiModule,
    limit: u32,
) -> Result<WasmiModule, WasmValidationError> {
    if let Some(sect) = module.table_section_mut() {
        // Table section is optional and there can be at most one.
        if sect.entries().len() > 1 {
            return Err(WasmValidationError::MoreThanOneTable);
        }

        if let Some(table_entry) = sect.entries_mut().first_mut() {
            let initial = table_entry.limits().initial();
            if initial > limit {
                return Err(WasmValidationError::InitialTableSizeExceeded {
                    max: limit,
                    actual: initial,
                });
            }

            match table_entry.limits().maximum() {
                Some(max) => {
                    if max > limit {
                        return Err(WasmValidationError::MaxTableSizeExceeded {
                            max: limit,
                            actual: max,
                        });
                    }
                }
                None => {
                    // rewrite wasm and provide a maximum limit for a table section
                    *table_entry = TableType::new(initial, Some(limit))
                }
            }
        }
    }

    Ok(module)
}

/// Ensure that any `br_table` instruction adheres to its immediate value limit.
fn ensure_br_table_size_limit(module: &WasmiModule, limit: u32) -> Result<(), WasmValidationError> {
    let code_section = if let Some(type_section) = module.code_section() {
        type_section
    } else {
        return Ok(());
    };
    for instr in code_section
        .bodies()
        .iter()
        .flat_map(|body| body.code().elements())
    {
        if let Instruction::BrTable(br_table_data) = instr {
            if br_table_data.table.len() > limit as usize {
                return Err(WasmValidationError::BrTableSizeExceeded {
                    max: limit,
                    actual: br_table_data.table.len(),
                });
            }
        }
    }
    Ok(())
}

/// Ensures that module doesn't declare too many globals.
///
/// Globals are not limited through the `stack_height` as locals are. Neither does
/// the linear memory limit `memory_pages` applies to them.
fn ensure_global_variable_limit(
    module: &WasmiModule,
    limit: u32,
) -> Result<(), WasmValidationError> {
    if let Some(global_section) = module.global_section() {
        let actual = global_section.entries().len();
        if actual > limit as usize {
            return Err(WasmValidationError::TooManyGlobals { max: limit, actual });
        }
    }
    Ok(())
}

/// Ensure maximum numbers of parameters a function can have.
///
/// Those need to be limited to prevent a potentially exploitable interaction with
/// the stack height instrumentation: The costs of executing the stack height
/// instrumentation for an indirectly called function scales linearly with the amount
/// of parameters of this function. Because the stack height instrumentation itself is
/// is not weight metered its costs must be static (via this limit) and included in
/// the costs of the instructions that cause them (call, call_indirect).
fn ensure_parameter_limit(module: &WasmiModule, limit: u32) -> Result<(), WasmValidationError> {
    let type_section = if let Some(type_section) = module.type_section() {
        type_section
    } else {
        return Ok(());
    };

    for Type::Function(func) in type_section.types() {
        let actual = func.params().len();
        if actual > limit as usize {
            return Err(WasmValidationError::TooManyParameters { max: limit, actual });
        }
    }

    Ok(())
}

/// Ensures that Wasm module has valid imports.
fn ensure_valid_imports(module: &WasmiModule) -> Result<(), WasmValidationError> {
    let import_entries = module
        .import_section()
        .map(|is| is.entries())
        .unwrap_or(&[]);

    // Gas counter is currently considered an implementation detail.
    //
    // If a wasm module tries to import it will be rejected.

    for import in import_entries {
        if import.module() == DEFAULT_GAS_MODULE_NAME
            && import.field() == INTERNAL_GAS_FUNCTION_NAME
        {
            return Err(WasmValidationError::MissingHostFunction);
        }
    }

    Ok(())
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

// NOTE: Imitates persistent on disk cache for precompiled artifacts, ideally it should be something
// like `Box<dyn ArtifactsCache>` and stick it to WasmEngine, so for instance while testing we can
// have a global cache instance to speed up testing.
#[derive(PartialEq, Eq, Hash)]
struct CacheKey(
    /// When cache is implemented as persistent on disk it's important to note that wasmer can use
    /// host CPU's features that may not be present when persistent cache is moved to different CPU
    /// with missing feature sets.
    wasmer::Triple,
    /// We're caching instrumented binary keyed by the original wasm bytes therefore we also need
    /// to know the wasm config that's used to instrument the wasm bytes. After chainspec will
    /// modify OpcodeCosts we need to create new cache entry to avoid incorrect execution of old
    /// binary but under new chain configuration.
    WasmConfig,
    /// Raw Wasm bytes without any modification.
    Bytes,
);

static GLOBAL_CACHE: Lazy<Mutex<HashMap<CacheKey, Bytes>>> = Lazy::new(Default::default);

fn cache_get(correlation_id: &CorrelationId, cache_key: &CacheKey) -> Option<Bytes> {
    let hash_map = GLOBAL_CACHE.lock().unwrap();
    let precompiled = hash_map.get(cache_key)?;

    Some(precompiled.clone())
}

fn cache_set(correlation_id: &CorrelationId, cache_key: CacheKey, artifact: Bytes) {
    let mut global_cache = GLOBAL_CACHE.lock().unwrap();
    let res = global_cache.insert(cache_key, artifact);
    assert!(res.is_none())
}

/// Wasm preprocessor.
#[derive(Debug, Clone)]
pub struct WasmEngine {
    wasm_config: WasmConfig,
    execution_mode: ExecutionMode,
    compiled_engine: Arc<WasmtimeEngine>,
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

    // TODO: Tweak more
    let wasmtime_engine = wasmtime::Engine::new(&config).expect("should create new engine");
    WasmtimeEngine(wasmtime_engine)
}

fn make_wasmer_store(backend: WasmerBackend) -> wasmer::Store {
    let engine: wasmer::Engine = match backend {
        WasmerBackend::Singlepass => Singlepass::new().into(),
        WasmerBackend::Cranelift { optimize } => {
            let mut cranelift = Cranelift::new();
            match optimize {
                CraneliftOptLevel::None => {
                    cranelift.opt_level(wasmer::CraneliftOptLevel::None);
                }
                CraneliftOptLevel::Speed => {
                    cranelift.opt_level(wasmer::CraneliftOptLevel::Speed);
                }
                CraneliftOptLevel::SpeedAndSize => {
                    cranelift.opt_level(wasmer::CraneliftOptLevel::SpeedAndSize);
                }
            }
            cranelift.into()
        }
    };
    wasmer::Store::new(engine)
}

impl WasmEngine {
    /// Get a reference to the wasm engine's compiled engine.
    pub fn compiled_engine(&self) -> &wasmtime::Engine {
        &self.compiled_engine
    }

    fn deserialize_compiled(&self, bytes: &[u8]) -> Result<wasmtime::Module, execution::Error> {
        let compiled_module =
            unsafe { wasmtime::Module::deserialize(&self.compiled_engine(), bytes) }.unwrap();
        Ok(compiled_module)
    }

    pub fn execution_mode(&self) -> &ExecutionMode {
        &self.execution_mode
    }

    pub fn execution_mode_mut(&mut self) -> &mut ExecutionMode {
        &mut self.execution_mode
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
    pub fn instance_and_memory<R>(
        &self,
        wasm_module: Module,
        protocol_version: ProtocolVersion,
        runtime: Runtime<R>,
    ) -> Result<Instance<R>, execution::Error>
    where
        R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
        R::Error: Into<execution::Error>,
    {
        // match wasm_engine.execution_mode() {
        match wasm_module {
            Module::Noop => Ok(Instance::Noop),
            Module::Interpreted {
                original_bytes,
                wasmi_module,
                timestamp,
            } => {
                let module = wasmi::Module::from_parity_wasm_module(wasmi_module.clone())
                    .map_err(|e| execution::Error::Interpreter(e.to_string()))?;
                // let resolver = create_module_resolver(protocol_version, self.wasm_config())?;
                let mut imports = ImportsBuilder::new();
                let wasmi_resolver = WasmiResolver::new(self.wasm_config().max_memory);
                imports.push_resolver("env", &wasmi_resolver);
                let not_started_module = ModuleInstance::new(&module, &imports)
                    .map_err(|e| execution::Error::Interpreter(e.to_string()))?;
                if not_started_module.has_start() {
                    return Err(execution::Error::UnsupportedWasmStart);
                }
                let instance = not_started_module.not_started_instance().clone();
                let memory = wasmi_resolver.memory().unwrap();

                Ok(Instance::Interpreted {
                    original_bytes: original_bytes.clone(),
                    module: instance,
                    memory: memory.clone(),
                    runtime,
                    timestamp,
                })
            }
            Module::Compiled {
                original_bytes,
                wasmi_module,
                // compiled_artifact,
                wasmtime_module: compiled_module,
                precompile_time,
                // precompile_time,
            } => {
                // let compiled_module = self.deserialize_compiled(&compiled_artifact)?;

                // NOTE: This is duplicated with wasmi's memory resolver in v1_resolver.rs
                // "Module requested too much memory" is not runtime error it should be a validation
                // pass.
                let descriptor = compiled_module
                    .imports()
                    .filter_map(|import| import.ty().memory().cloned())
                    .nth(0)
                    .expect("should have memory");

                let max_memory = self.wasm_config().max_memory as u64;
                let descriptor_max = descriptor
                    .maximum()
                    .map(|pages| pages)
                    .unwrap_or(max_memory);
                // Checks if wasm's memory entry has too much initial memory or non-default max
                // memory pages exceeds the limit.
                if descriptor.minimum() > descriptor_max || descriptor_max > max_memory {
                    return Err(execution::Error::Interpreter(
                        "Module requested too much memory".into(),
                    ));
                }

                Ok(Instance::Compiled {
                    original_bytes: original_bytes.clone(),
                    module: wasmi_module.clone(),
                    compiled_module,
                    precompile_time: precompile_time.clone(),
                    runtime,
                })
            }
            Module::Jitted {
                original_bytes,
                wasmi_module,
                // compiled_artifact,
                precompile_time,
                module: compiled_module,
                // runtime,
            } => {
                // NOTE: This is duplicated with wasmi's memory resolver in v1_resolver.rs
                // "Module requested too much memory" is not runtime error it should be a validation
                // pass.
                let descriptor = compiled_module
                    .imports()
                    .filter_map(|import| import.ty().memory().cloned())
                    .nth(0)
                    .expect("should have memory");

                let max_memory = self.wasm_config().max_memory as u64;
                let descriptor_max = descriptor
                    .maximum()
                    .map(|pages| pages)
                    .unwrap_or(max_memory);
                // Checks if wasm's memory entry has too much initial memory or non-default max
                // memory pages exceeds the limit.
                if descriptor.minimum() > descriptor_max || descriptor_max > max_memory {
                    return Err(execution::Error::Interpreter(
                        "Module requested too much memory".into(),
                    ));
                }

                // aot compile
                // let precompiled_bytes =
                // self.compiled_engine.precompile_module(&preprocessed_wasm_bytes).expect("should
                // preprocess"); Ok(Module::Compiled(precompiled_bytes))

                // todo!("compiled mode")
                // let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine(), ());
                // let instance = wasmtime::Instance::new(&mut store, &compiled_module,
                // &[]).expect("should create compiled module");

                // let compiled_module = self.deserialize_compiled(&compiled_artifact)?;
                Ok(Instance::Compiled {
                    original_bytes: original_bytes.clone(),
                    module: wasmi_module.clone(),
                    compiled_module: compiled_module.clone(),
                    precompile_time: precompile_time.clone(),
                    runtime,
                })
            }
            Module::Singlepass {
                original_bytes,
                wasmi_module,
                wasmer_module,
                store,
                timestamp,
            } => {
                let max_memory = self.wasm_config().max_memory;

                // NOTE: This is duplicated with wasmi's memory resolver in v1_resolver.rs
                // "Module requested too much memory" is not runtime error it should be a validation
                // pass.
                let descriptor = wasmer_module
                    .imports()
                    .memories()
                    .map(|import_type| import_type.ty().clone())
                    .nth(0)
                    .unwrap_or_else(|| wasmer::MemoryType::new(max_memory, Some(max_memory), true));

                let descriptor_max = descriptor
                    .maximum
                    .map(|pages| pages.0)
                    .unwrap_or(max_memory);
                // Checks if wasm's memory entry has too much initial memory or non-default max
                // memory pages exceeds the limit.
                if descriptor.minimum.0 > descriptor_max || descriptor_max > max_memory {
                    return Err(execution::Error::Interpreter(
                        "Module requested too much memory".into(),
                    ));
                }

                Ok(Instance::Singlepass {
                    original_bytes: original_bytes.clone(),
                    module: wasmi_module.clone(),
                    wasmer_module: wasmer_module,
                    runtime,
                    store: store,
                    timestamp,
                })
            }
        }
    }

    fn make_cache_key(&self, original_bytes: Bytes) -> CacheKey {
        CacheKey(wasmer::Triple::host(), self.wasm_config, original_bytes)
    }

    fn cache_get(&self, correlation_id: &CorrelationId, original_bytes: Bytes) -> Option<Bytes> {
        cache_get(correlation_id, &self.make_cache_key(original_bytes))
    }

    fn cache_set(&self, correlation_id: &CorrelationId, clone: Bytes, serialized_bytes: Bytes) {
        cache_set(correlation_id, self.make_cache_key(clone), serialized_bytes)
    }

    /// Creates module specific for execution mode.
    pub fn module_from_bytes(
        &self,
        correlation_id: CorrelationId,
        wasm_bytes: Bytes,
    ) -> Result<Module, PreprocessingError> {
        let start = Instant::now();
        let parity_module = deserialize_interpreted(&wasm_bytes)?;

        let module = match self.execution_mode {
            ExecutionMode::Interpreted => Module::Interpreted {
                wasmi_module: parity_module,
                original_bytes: wasm_bytes,
                timestamp: start,
            },
            ExecutionMode::Compiled { cache_artifacts } => {
                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(&correlation_id, wasm_bytes.clone())
                    {
                        correlation_id.record_property(Property::VMCacheHit {
                            original_bytes: wasm_bytes.clone(),
                        });
                        let wasmtime_module = self
                            .deserialize_compiled(&precompiled_bytes)
                            .expect("Should deserialize");
                        return Ok(Module::Compiled {
                            original_bytes: wasm_bytes,
                            wasmi_module: parity_module,
                            precompile_time: None,
                            wasmtime_module: wasmtime_module,
                        });
                    }
                }
                // aot compile
                // let (precompiled_bytes, duration) = self
                //     .precompile(wasm_bytes.clone(), wasm_bytes.clone(), cache_artifacts)
                //     .unwrap();
                // self.compiled_engine.precompile_module(&wasm_bytes).expect("should preprocess");

                correlation_id.record_property(Property::VMCacheMiss {
                    original_bytes: wasm_bytes.clone(),
                });
                let module = wasmtime::Module::new(&self.compiled_engine, &wasm_bytes).unwrap();

                if cache_artifacts {
                    let serialized_bytes = module
                        .serialize()
                        .expect("should serialize wasmtime module");
                    self.cache_set(&correlation_id, wasm_bytes.clone(), serialized_bytes.into());
                }

                Module::Compiled {
                    original_bytes: wasm_bytes,
                    wasmi_module: parity_module,
                    precompile_time: None,
                    wasmtime_module: module,
                    // compiled_artifact: precompiled_bytes.to_owned(),
                }
            }
            ExecutionMode::JustInTime => {
                let start = Instant::now();
                let module =
                    wasmtime::Module::new(&self.compiled_engine, wasm_bytes.clone()).unwrap();
                let stop = start.elapsed();
                Module::Jitted {
                    original_bytes: wasm_bytes,
                    wasmi_module: parity_module,
                    precompile_time: Some(stop),
                    module,
                }
            }
            ExecutionMode::Wasmer {
                backend,
                cache_artifacts,
            } => {
                let hash_digest = base16::encode_lower(&blake2b(&wasm_bytes));
                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(&correlation_id, wasm_bytes.clone())
                    {
                        // We have the artifact, and we can use headless mode to just execute binary
                        // artifact
                        let headless_engine = wasmer::Engine::headless();
                        let store = wasmer::Store::new(headless_engine);
                        let wasmer_module = unsafe {
                            wasmer::Module::deserialize(&store, precompiled_bytes.clone())
                        }?;
                        // let wasmer_imports_from_cache =
                        //     wasmer_module.imports().into_iter().collect::<Vec<_>>();

                        return Ok(Module::Singlepass {
                            original_bytes: wasm_bytes,
                            wasmi_module: parity_module,
                            wasmer_module,
                            store: store,
                            timestamp: start,
                        });
                    }
                }

                let wasmer_store = make_wasmer_store(backend);

                let wasmer_module = wasmer::Module::from_binary(&wasmer_store, &wasm_bytes)
                    .expect("should create wasmer module");

                if cache_artifacts {
                    correlation_id.record_property(Property::VMCacheMiss {
                        original_bytes: wasm_bytes.clone(),
                    });
                    let serialized_bytes = wasmer_module.serialize()?;
                    self.cache_set(&correlation_id, wasm_bytes.clone(), serialized_bytes);
                }

                // TODO: Cache key should be the one from StoredValue::ContractWasm which is random,
                // there's probably collision and somehow unmodified module is cached?

                Module::Singlepass {
                    original_bytes: wasm_bytes,
                    wasmi_module: parity_module,
                    wasmer_module: wasmer_module,
                    store: wasmer_store,
                    timestamp: start,
                }
            }
        };
        Ok(module)
    }

    /// Creates a new instance of the preprocessor.
    pub fn new(wasm_config: WasmConfig) -> Self {
        Self {
            wasm_config,
            execution_mode: wasm_config.execution_mode,
            compiled_engine: Arc::new(new_compiled_engine(&wasm_config)),
        }
    }
    // fn precompile(
    //     &self,
    //     original_bytes: Bytes,
    //     bytes: Bytes,
    //     cache_artifacts: bool,
    // ) -> Result<(Bytes, Option<Duration>), RuntimeError> {
    //     // if cache_artifacts {
    //     //     if let Some(precompiled) = self.cache_get(&correlation_id, original_bytes.clone())
    // {     //         let deserialized_module =
    //     //             unsafe { wasmtime::Module::deserialize(&self.compiled_engine,
    // &precompiled) }     //                 .expect("should deserialize wasmtime module");
    //     //         return Ok(());
    //     //     }
    //     // }
    //     // todo!("precompile; disabled because of trait issues colliding with wasmer");
    //     // let mut cache = GLOBAL_CACHE.lock().unwrap();
    //     // let mut cache = cache.borrow_mut();
    //     // let mut cache = GLOBAL_CACHE.lborrow_mut();
    //     // match self.cache_get(&correlation_id, bytes)

    //     // let (bytes, maybe_duration) = match cache.entry(CacheKey(bytes.clone())) {
    //     //     Entry::Occupied(o) => (o.get().clone(), None),
    //     //     Entry::Vacant(v) => {
    //     let start = Instant::now();
    //     let precompiled_bytes = self
    //         .compiled_engine
    //         .precompile_module(&bytes)
    //         .map_err(|e| RuntimeError::Other(e.to_string()))?;
    //     let stop = start.elapsed();
    //     //         // record_performance_log(original_bytes, LogProperty::PrecompileTime(stop));
    //     //         let artifact = v.insert(Bytes::from(precompiled_bytes)).clone();
    //     //         (artifact, Some(stop))
    //     //     }
    //     // }
    //     Ok((precompiled_bytes.into(), Some(stop)))
    // }

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
    pub fn preprocess(
        &self,
        correlation_id: CorrelationId,
        wasm_config: WasmConfig,
        module_bytes: &[u8],
    ) -> Result<Module, PreprocessingError> {
        let start = Instant::now();

        // NOTE: Consider using `bytes::Bytes` instead of `bytesrepr::Bytes` as its cheaper
        let module_bytes = Bytes::copy_from_slice(module_bytes);

        correlation_id.record_property(Property::Preprocess {
            original_bytes: module_bytes.clone(),
        });

        let module = deserialize_interpreted(&module_bytes)?;

        ensure_valid_access(&module)?;

        if module.start_section().is_some() {
            // Remove execution::Error::UnsupportedWasmStart as previously with wasmi it was raised
            // when we have module instance to run, but with other backends we'd have to expend a
            // lot of resource to first compile artifact just to validate it - we can safely exit
            // early without doing extra work.
            return Err(WasmValidationError::UnsupportedWasmStart.into());
            // return Err(execution::Error::UnsupportedWasmStart);
        }

        if memory_section(&module).is_none() {
            // `pwasm_utils::externalize_mem` expects a non-empty memory section to exist in the
            // module, and panics otherwise.
            return Err(PreprocessingError::MissingMemorySection);
        }

        let module = ensure_table_size_limit(module, DEFAULT_MAX_TABLE_SIZE)?;
        ensure_br_table_size_limit(&module, DEFAULT_BR_TABLE_MAX_SIZE)?;
        ensure_global_variable_limit(&module, DEFAULT_MAX_GLOBALS)?;
        ensure_parameter_limit(&module, DEFAULT_MAX_PARAMETER_COUNT)?;
        ensure_valid_imports(&module)?;

        let module = pwasm_utils::externalize_mem(module, None, wasm_config.max_memory);
        let module = pwasm_utils::inject_gas_counter(
            module,
            &wasm_config.opcode_costs().to_set(),
            DEFAULT_GAS_MODULE_NAME,
        )
        .map_err(|_| PreprocessingError::OperationForbiddenByGasRules)?;
        let module = stack_height::inject_limiter(module, wasm_config.max_stack_height)
            .map_err(|_| PreprocessingError::StackLimiter)?;

        if self.execution_mode.is_singlepass() && self.execution_mode.is_using_cache() {
            if let Some(precompiled_bytes) = self.cache_get(&correlation_id, module_bytes.clone()) {
                // We have the artifact, and we can use headless mode to just execute binary
                // artifact
                let headless_engine = wasmer::Engine::headless();
                let store = wasmer::Store::new(headless_engine);
                let wasmer_module =
                    unsafe { wasmer::Module::deserialize(&store, precompiled_bytes.clone()) }?;

                return Ok(Module::Singlepass {
                    original_bytes: module_bytes,
                    wasmi_module: module,
                    wasmer_module,
                    store: store,
                    timestamp: start,
                });
            }
        }
        match self.execution_mode {
            ExecutionMode::Interpreted => Ok(Module::Interpreted {
                original_bytes: module_bytes,
                wasmi_module: module,
                timestamp: start,
            }),
            ExecutionMode::Compiled { cache_artifacts } => {
                // TODO: Gas injected module is used here but we might want to use `module` instead
                // with other preprocessing done.
                let preprocessed_wasm_bytes = Bytes::from(
                    parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes"),
                );

                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(&correlation_id, module_bytes.clone())
                    {
                        correlation_id.record_property(Property::VMCacheHit {
                            original_bytes: module_bytes.clone(),
                        });
                        let wasmtime_module = unsafe {
                            wasmtime::Module::deserialize(&self.compiled_engine, &precompiled_bytes)
                        }
                        .expect("should deserialize wasmtime module");
                        return Ok(Module::Compiled {
                            original_bytes: module_bytes,
                            wasmi_module: module,
                            precompile_time: None,
                            wasmtime_module,
                        });
                    }
                }

                let wasmtime_module =
                    wasmtime::Module::new(&self.compiled_engine, &preprocessed_wasm_bytes)
                        .map_err(|error| PreprocessingError::Precompile(error.to_string()))?;

                if cache_artifacts {
                    correlation_id.record_property(Property::VMCacheMiss {
                        original_bytes: module_bytes.clone(),
                    });
                    let serialized_bytes = wasmtime::Module::serialize(&wasmtime_module).unwrap();
                    self.cache_set(
                        &correlation_id,
                        module_bytes.clone(),
                        serialized_bytes.into(),
                    );
                }
                // aot compile
                // let (precompiled_bytes, duration) = self
                //     .precompile(
                //         module_bytes.clone(),
                //         preprocessed_wasm_bytes,
                //         cache_artifacts,
                //     )
                //     .map_err(PreprocessingError::Precompile)?;

                Ok(Module::Compiled {
                    original_bytes: module_bytes,
                    wasmi_module: module,
                    precompile_time: None,
                    wasmtime_module,
                    // compiled_artifact: precompiled_bytes,
                })
            }
            ExecutionMode::JustInTime => {
                let start = Instant::now();

                let preprocessed_wasm_bytes =
                    parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes");
                let compiled_module =
                    wasmtime::Module::new(&self.compiled_engine, preprocessed_wasm_bytes)
                        .map_err(|e| PreprocessingError::Deserialize(e.to_string()))?;

                let stop = start.elapsed();

                Ok(Module::Jitted {
                    original_bytes: module_bytes,
                    wasmi_module: module,
                    precompile_time: Some(stop),
                    module: compiled_module,
                })
            }
            ExecutionMode::Wasmer {
                backend,
                cache_artifacts,
            } => {
                let preprocessed_wasm_bytes =
                    parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes");

                let start = Instant::now();

                let mut store = make_wasmer_store(backend);

                let wasmer_module =
                    wasmer::Module::from_binary(&mut store, &preprocessed_wasm_bytes)?;

                if cache_artifacts {
                    correlation_id.record_property(Property::VMCacheMiss {
                        original_bytes: module_bytes.clone(),
                    });
                    let serialized_bytes = wasmer_module.serialize()?;
                    self.cache_set(&correlation_id, module_bytes.clone(), serialized_bytes);
                }

                let stop = start.elapsed();

                Ok(Module::Singlepass {
                    original_bytes: module_bytes,
                    wasmi_module: module,
                    store,
                    wasmer_module: wasmer_module,
                    timestamp: start,
                })
            }
        }
        // let module = deserialize_interpreted(module_bytes)?;
    }

    /// Get a reference to the wasm engine's wasm config.
    pub fn wasm_config(&self) -> &WasmConfig {
        &self.wasm_config
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn wasmtime_trap_recover() {
//         let error = execution::Error::Revert(ApiError::User(100));
//         let trap: wasmtime::Trap = wasmtime::Trap::from(error);
//         let runtime_error = RuntimeError::from(trap);
//         let recovered = runtime_error
//             .into_execution_error()
//             .expect("should have error");
//         assert!(matches!(
//             recovered,
//             execution::Error::Revert(ApiError::User(100))
//         ));
//     }
// }
