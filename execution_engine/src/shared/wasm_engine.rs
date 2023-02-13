//! Preprocessing of Wasm modules.
use bytes::Bytes;
use casper_types::{
    blake2b,
    bytesrepr::{self, FromBytes, ToBytes},
};

use once_cell::sync::Lazy;
use parity_wasm::elements::{
    self, External, Instruction, Internal, MemorySection, Section, TableType, Type,
};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    collections::HashMap,
    convert::TryInto,
    error::Error,
    fmt::{self, Formatter},
    fs,
    ops::Deref,
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use thiserror::Error;
use wasmer::{AsStoreMut, AsStoreRef, CompilerConfig, Cranelift, FunctionEnv, TypedFunction};
use wasmer_compiler_singlepass::Singlepass;
use wasmer_middlewares::metering::{self, MeteringPoints};
use wasmi::WasmType;

pub mod host_interface;
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
// use wasmi::{ImportsBuilder, MemoryRef, ModuleInstance, ModuleRef};
use wasmtime::{
    AsContext, AsContextMut, Caller, Extern, ExternType, InstanceAllocationStrategy, Memory,
    MemoryType, StoreContextMut, Trap,
};

use crate::core::execution;

use self::{
    host_interface::WasmHostInterface,
    wasmer_backend::{make_wasmer_backend, WasmerAdapter, WasmerEnv},
    wasmi_backend::{make_wasmi_linker, WasmiEngine, WasmiEnv},
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstrumentMode {
    None,
    ParityWasm,
    MeteringMiddleware,
}

impl Default for InstrumentMode {
    fn default() -> Self {
        Self::None
    }
}
/// Mode of execution for smart contracts.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct Wasmer {
    backend: WasmerBackend,
    cache_artifacts: bool,
    instrument: InstrumentMode,
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
    Wasmer(Wasmer),
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
            ExecutionMode::Wasmer(Wasmer {
                cache_artifacts, ..
            }) => *cache_artifacts,
        }
    }

    pub fn as_wasmer(&self) -> Option<&Wasmer> {
        if let Self::Wasmer(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

impl ToBytes for ExecutionMode {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        match self {
            ExecutionMode::Interpreted => 1u32.to_bytes(),
            ExecutionMode::Compiled { cache_artifacts } => (2u32, *cache_artifacts).to_bytes(),
            ExecutionMode::JustInTime => 3u32.to_bytes(),
            ExecutionMode::Wasmer(Wasmer {
                cache_artifacts, ..
            }) => (4u32, *cache_artifacts).to_bytes(),
        }
    }

    fn serialized_length(&self) -> usize {
        match self {
            ExecutionMode::Interpreted => 1u32.serialized_length(),
            ExecutionMode::Compiled { cache_artifacts } => {
                (2u32, *cache_artifacts).serialized_length()
            }
            ExecutionMode::JustInTime => 3u32.serialized_length(),
            ExecutionMode::Wasmer(Wasmer {
                backend,
                cache_artifacts,
                ..
            }) => (4u32, *cache_artifacts).serialized_length(),
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

pub(crate) fn deserialize_interpreted(
    wasm_bytes: &[u8],
) -> Result<WasmiModule, PreprocessingError> {
    parity_wasm::elements::deserialize_buffer(wasm_bytes).map_err(PreprocessingError::from)
}
pub(crate) fn serialize_interpreted(module: WasmiModule) -> Result<Vec<u8>, PreprocessingError> {
    parity_wasm::elements::serialize(module).map_err(PreprocessingError::from)
}

pub(crate) fn instrument_module(
    module: WasmiModule,
    wasm_config: &WasmConfig,
) -> Result<WasmiModule, PreprocessingError> {
    // let module = deserialize_interpreted(module_bytes)?;
    ensure_valid_access(&module)?;
    if module.start_section().is_some() {
        // Remove execution::Error::UnsupportedWasmStart as previously with wasmi it was raised
        // when we have module instance to run, but with other backends we'd have to expend a
        // lot of resource to first compile artifact just to validate it - we can safely exit
        // early without doing extra work.
        return Err(WasmValidationError::UnsupportedWasmStart.into());
        // return Err(RuntimeError::UnsupportedWasmStart);
    }
    if memory_section(&module).is_none() {
        // `pwasm_utils::externalize_mem` expects a non-empty memory section to exist in the
        // module, and panics otherwise.
        // dbg!(&module);
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
    Ok(module)
}

/// Statically dispatched Wasm module wrapper for different implementations
/// NOTE: Probably at this stage it can hold raw wasm bytes without being an enum, and the decision
/// can be made later
pub enum Module {
    Interpreted {
        original_bytes: Bytes,
        module: wasmi::Module,
        timestamp: Instant,
        engine: WasmiEngine,
    },
    Compiled {
        original_bytes: Bytes,
        precompile_time: Option<Duration>,
        /// Ahead of time compiled artifact.
        // compiled_artifact: Bytes,
        wasmtime_module: wasmtime::Module,
    },
    Jitted {
        original_bytes: Bytes,
        precompile_time: Option<Duration>,
        module: wasmtime::Module,
    },
    Singlepass {
        original_bytes: Bytes,
        wasmer_module: wasmer::Module,
        store: wasmer::Store,
        timestamp: Instant,
    },
}
impl Module {
    pub(crate) fn get_original_bytes(&self) -> &Bytes {
        match self {
            Module::Interpreted { original_bytes, .. } => original_bytes,
            Module::Compiled { original_bytes, .. } => original_bytes,
            Module::Jitted { original_bytes, .. } => original_bytes,
            Module::Singlepass { original_bytes, .. } => original_bytes,
        }
    }
}

impl fmt::Debug for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interpreted { .. } => f.debug_tuple("Interpreted").finish(),
            Self::Compiled { .. } => f.debug_tuple("Compiled").finish(),
            Self::Jitted { .. } => f.debug_tuple("Jitted").finish(),
            Self::Singlepass { .. } => f.debug_tuple("Singlepass").finish(),
        }
    }
}

/// Common error type adapter for all Wasm engines supported.
#[derive(Error, Clone, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    WasmiError(Arc<wasmi::Error>),
    #[error(transparent)]
    WasmiTrap(Arc<wasmi::core::Trap>),
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
    #[error("unsupported wasm start")]
    UnsupportedWasmStart,
    #[error("module requested too much memory")]
    ModuleRequestedTooMuchMemory,
    #[error("{0}")]
    WasmiMemoryError(Arc<wasmi::errors::MemoryError>),
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
    pub fn into_host_error<T: std::error::Error + Clone + Send + Sync + 'static>(
        self,
    ) -> Result<T, Self> {
        match &self {
            RuntimeError::WasmiError(wasmi_error) => {
                // todo!()
                // match wasmi_error.as_host_error().and_then(|host_error| {
                //     host_error.
                // downcast_ref::<wasmi_backend::IndirectHostError<T>>()
                // }) {
                //     Some(indirect) => Ok(indirect.source.clone()),
                //     None => Err(self),
                // }
                // Ok(error)

                match wasmi_error.deref() {
                    wasmi::Error::Trap(trap) => {
                        let maybe_error: Option<&wasmi_backend::IndirectHostError<T>> =
                            trap.downcast_ref();

                        match maybe_error {
                            Some(error) => return Ok(error.source.clone()),
                            None => return Err(self),
                        }
                    }
                    _ => Err(self),
                }
            }
            RuntimeError::WasmiTrap(wasmi_trap) => {
                // todo!()
                // match wasmi_error.as_host_error().and_then(|host_error| {
                //     host_error.
                // downcast_ref::<wasmi_backend::IndirectHostError<execution::Error>>()
                // }) {
                //     Some(indirect) => Ok(indirect.source.clone()),
                //     None => Err(self),
                // }
                // Ok(error)
                let maybe_error: Option<&wasmi_backend::IndirectHostError<T>> =
                    wasmi_trap.downcast_ref();
                match maybe_error {
                    Some(error) => return Ok(error.source.clone()),
                    None => return Err(self),
                }
            }
            RuntimeError::WasmtimeError(wasmtime_trap) => {
                match wasmtime_trap
                    .source()
                    .and_then(|src| src.downcast_ref::<T>())
                {
                    Some(execution_error) => Ok(execution_error.clone()),
                    None => Err(self),
                }
            }
            RuntimeError::WasmerError(wasmer_runtime_error) => {
                match wasmer_runtime_error
                    // .clone()
                    .source()
                    .and_then(|src| src.downcast_ref::<T>())
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

/// Warmed up instance of a wasm module used for execution.
pub enum Instance<H>
where
    H: WasmHostInterface, /* H: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
                           * H::Error: Into<execution::Error>, */
{
    Interpreted {
        original_bytes: Bytes,
        // module: wasmi::Module,
        instance: wasmi::Instance,
        // runtime: Runtime<H>,
        // runtime: H,
        timestamp: Instant,
        engine: WasmiEngine,
        store: wasmi::Store<WasmiEnv<H>>,
        // interpreted_engine: WasmiEngine,
    },
    // NOTE: Instance should contain wasmtime::Instance instead but we need to hold Store that has
    // a lifetime and a generic R
    Compiled {
        /// For metrics
        original_bytes: Bytes,
        precompile_time: Option<Duration>,
        /// Raw Wasmi module used only for further processing.
        // module: WasmiModule,
        /// This is compiled module used for execution.
        // compiled_module: wasmtime::Module,
        instance: wasmtime::Instance,
        store: wasmtime::Store<WasmtimeEnv<H>>,
        // runtime: Runtime<H>,
        // runtime: H,
    },
    Singlepass {
        original_bytes: Bytes,
        // module: WasmiModule,
        /// There is no separate "compile" step that turns a wasm bytes into a wasmer module
        instance: wasmer::Instance,
        // runtime: Runtime<H>,
        // runtime: H,
        store: wasmer::Store,
        timestamp: Instant,
        preprocess_dur: Duration,
    },
}

// unsafe impl<R> Send for Instance<R>
// where
//     R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
//     R::Error: Into<execution::Error>,
// {
// }
// unsafe impl<R> Sync for Instance<R> where R: Sync {}

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

// #[derive(Debug)]
// pub struct InvokeOutcome<Ret>
// where
//     Ret: fmt::Debug,
// {
//     pub return_value: Ret,
//     pub metering_points: Option<MeteringPoints>,
// }

// impl<Ret> InvokeOutcome<Ret>
// where
//     Ret: fmt::Debug,
// {
//     fn new(return_value: Ret, metering_points: MeteringPoints) -> Self {
//         Self {
//             return_value,
//             metering_points: Some(metering_points),
//         }
//     }
//     fn with_return_value(return_value: Ret) -> Self {
//         Self {
//             return_value,
//             metering_points: None,
//         }
//     }
// }

impl<H> Instance<H>
where
    H: WasmHostInterface + Send + Sync + 'static, /* H: Send + Sync + 'static + Clone +
                                                   * StateReader<Key, StoredValue>, */
    H::Error: std::error::Error,
{
    /// Invokes exported function
    pub fn invoke_export<Ret, T>(
        &mut self,
        correlation_id: Option<CorrelationId>,
        wasm_engine: &WasmEngine,
        func_name: &str,
        args: (),
    ) -> Result<Ret, RuntimeError>
    where
        // Temp: wasmi::WasmResults,
        // T: wasmi::WasmResults,
        // wasmi::WasmResults: Into<Ret>,
        // for<'a> T: wasmi::WasmResults,
        T: wasmi::WasmResults,
        Ret: fmt::Debug + From<T> + wasmer::WasmTypeList + wasmtime::WasmResults,
        // <Ret as Into<Ret>

        // wasmi::WasmResults: Into<Ret>
    {
        match self {
            Instance::Interpreted {
                original_bytes,
                instance,
                // memory,
                // runtime,
                timestamp,
                engine,
                // interpreted_engine,
                store,
            } => {
                let call_func = instance.get_typed_func::<(), T>(&store, func_name)?;
                let ret = call_func
                    .call(store, ())
                    .map_err(|trap| RuntimeError::WasmiTrap(Arc::new(trap)))?;
                let return_value: Ret = ret.into();

                Ok(return_value)
            }
            Instance::Compiled {
                original_bytes,
                // module,
                // mut compiled_module,
                instance,
                store,
                precompile_time,
                // runtime,
            } => {
                let exported_func = instance
                    .get_typed_func::<(), Ret, _>(store.as_context_mut(), func_name)
                    .expect("should get typed func");

                let ret = exported_func
                    .call(store.as_context_mut(), ())
                    .map_err(RuntimeError::from);

                // let stop = start.elapsed();

                let result = ret?;

                Ok(result)

                // Ok(Some(RuntimeValue::I64(0)))
            }
            Instance::Singlepass {
                original_bytes,
                // module,
                // wasmer_module,
                instance,
                // runtime,
                store,
                timestamp,
                preprocess_dur,
            } => {
                // let mut engine = wasm_engine.engine.as_wasmer().expect("valid config");

                // let store = wasmer::Store::new(engine.into());

                // let wasmer_store = wasm_engine.engine.as_wasmer_mut().expect("valid config");
                let call_func: TypedFunction<(), Ret> = instance
                    .exports
                    .get_typed_function(&store, func_name)
                    .expect("should get wasmer call func");

                let maybe_res = call_func.call(store);
                // dbg!(&maybe_res);
                let res = maybe_res?;

                let invoke_dur = timestamp.elapsed();

                if let Some(correlation_id) = correlation_id.as_ref() {
                    correlation_id.record_property(Property::WasmVM {
                        original_bytes: original_bytes.clone(),
                        preprocess_duration: preprocess_dur.clone(),
                        invoke_duration: invoke_dur,
                    });
                }

                // let outcome = if let ExecutionMode::Wasmer(Wasmer {
                //     instrument: InstrumentMode::MeteringMiddleware,
                //     ..
                // }) = wasm_engine.execution_mode()
                // {
                //     let metering = metering::get_remaining_points(&mut store, &instance);
                //     let outcome = InvokeOutcome::new(res, metering);
                //     eprintln!("outcome {:?}", outcome);
                //     outcome
                // } else {
                //     InvokeOutcome::with_return_value(res)
                // };

                Ok(res)
            }
        }
    }

    pub fn get_remaining_points(&mut self, wasm_engine: &WasmEngine) -> Option<MeteringPoints> {
        match self {
            Instance::Singlepass {
                instance, store, ..
            } => {
                if let ExecutionMode::Wasmer(Wasmer {
                    instrument: InstrumentMode::MeteringMiddleware,
                    ..
                }) = wasm_engine.execution_mode()
                {
                    let remaining_points =
                        metering::get_remaining_points(&mut store.as_store_mut(), &instance);
                    Some(remaining_points)
                } else {
                    None
                }

                // let outcome =
            }
            _ => None,
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
pub struct WasmtimeEngine(wasmtime::Engine);

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

fn cache_get(correlation_id: Option<&CorrelationId>, cache_key: &CacheKey) -> Option<Bytes> {
    let hash_map = GLOBAL_CACHE.lock().unwrap();
    let precompiled = hash_map.get(cache_key)?;

    Some(precompiled.clone())
}

fn cache_set(correlation_id: Option<&CorrelationId>, cache_key: CacheKey, artifact: Bytes) {
    let mut global_cache = GLOBAL_CACHE.lock().unwrap();
    let res = global_cache.insert(cache_key, artifact);
    assert!(res.is_none())
}

pub enum Engine {
    // Currently there seems to be issue where wasmi::Engine can't be reused when a wasm is trying
    // to invoke new wasm module,
    Wasmi,
    Wasmer,
    Wasmtime(WasmtimeEngine),
}

impl Engine {
    pub fn as_wasmtime(&self) -> Option<&WasmtimeEngine> {
        if let Self::Wasmtime(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

/// Wasm preprocessor.
#[derive(Clone)]
pub struct WasmEngine {
    wasm_config: WasmConfig,
    execution_mode: ExecutionMode,
    engine: Arc<Engine>,
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

impl WasmEngine {
    fn deserialize_compiled(&self, bytes: &[u8]) -> Result<wasmtime::Module, execution::Error> {
        let wasmtime_engine = self.engine.as_wasmtime().unwrap();
        let compiled_module =
            unsafe { wasmtime::Module::deserialize(wasmtime_engine, bytes) }.unwrap();
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
    pub fn instance_and_memory<H>(
        &self,
        wasm_module: Module,
        mut runtime: H,
    ) -> Result<Instance<H>, RuntimeError>
    where
        H: WasmHostInterface + Send + Sync + 'static,
        // R: Send + Sync + 'static + Clone + StateReader<Key, StoredValue>,
        H::Error: std::error::Error,
    {
        // match wasm_engine.execution_mode() {
        match wasm_module {
            Module::Interpreted {
                original_bytes,
                timestamp,
                module,
                engine,
            } => {
                let mut wasmi_env = WasmiEnv {
                    host: runtime,
                    memory: None,
                };

                let mut store = wasmi::Store::new(&engine, wasmi_env);

                let memory_import = module.imports().find_map(|import| {
                    if (import.module(), import.name()) == ("env", "memory") {
                        Some(import.ty().clone())
                    } else {
                        None
                    }
                });
                let memory_type = match memory_import {
                    Some(wasmi::ExternType::Memory(memory)) => memory.clone(),
                    Some(unknown_extern) => panic!("unexpected extern {:?}", unknown_extern),
                    None => wasmi::MemoryType::new(1, Some(self.wasm_config().max_memory)).unwrap(),
                };
                let memory = wasmi::Memory::new(&mut store, memory_type).unwrap();
                store.data_mut().memory = Some(memory);

                let mut linker = make_wasmi_linker("env", &mut store)
                    .map_err(|error| RuntimeError::Other(error.to_string()))?;
                linker
                    .define("env", "memory", wasmi::Extern::from(memory))
                    .unwrap();
                let instance = linker
                    .instantiate(&mut store, &module)?
                    .ensure_no_start(&mut store)
                    .expect("should ensure no start");

                Ok(Instance::Interpreted {
                    original_bytes: original_bytes.clone(),
                    instance,

                    timestamp,
                    engine,
                    store,
                })
            }
            Module::Compiled {
                original_bytes,
                // compiled_artifact,
                wasmtime_module: compiled_module,
                precompile_time,
                // precompile_time,
            } => {
                // let compiled_module = self.deserialize_compiled(&compiled_artifact)?;

                // NOTE: This is duplicated with wasmi's memory resolver in v1_resolver.rs
                // "Module requested too much memory" is not runtime error it should be a validation
                // pass.
                if let Some(descriptor) = compiled_module
                    .imports()
                    .filter_map(|import| import.ty().memory().cloned())
                    .nth(0)
                {
                    let max_memory = self.wasm_config().max_memory as u64;
                    let descriptor_max = descriptor
                        .maximum()
                        .map(|pages| pages)
                        .unwrap_or(max_memory);
                    // Checks if wasm's memory entry has too much initial memory or non-default max
                    // memory pages exceeds the limit.
                    if descriptor.minimum() > descriptor_max || descriptor_max > max_memory {
                        return Err(RuntimeError::ModuleRequestedTooMuchMemory);
                    }
                }

                // record_performance_log(&original_bytes,
                // LogProperty::EntryPoint(func_name.to_string()));

                let mut wasmtime_env = WasmtimeEnv {
                    host: runtime,
                    wasmtime_memory: None,
                };

                let wasmtime_engine = self.engine.as_wasmtime().expect("invalid configuration");
                let mut store = wasmtime::Store::new(wasmtime_engine, wasmtime_env);

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
                    None => MemoryType::new(1, Some(self.wasm_config().max_memory)),
                };

                // debug_assert_eq!(
                //     memory_type.maximum(),
                //     Some(wasm_engine.wasm_config().max_memory as u64)
                // );

                let memory = wasmtime::Memory::new(&mut store, memory_type).unwrap();
                store.data_mut().wasmtime_memory = Some(memory);
                let mut linker = wasmtime_backend::make_linker_object(
                    "env",
                    &self.engine.as_wasmtime().expect("valid config"),
                );
                linker
                    .define("env", "memory", wasmtime::Extern::from(memory))
                    .unwrap();

                let start = Instant::now();

                let instance = linker
                    .instantiate(&mut store, &compiled_module)
                    .map_err(|error| RuntimeError::Other(error.to_string()))?;

                Ok(Instance::Compiled {
                    original_bytes: original_bytes.clone(),
                    instance,
                    precompile_time: precompile_time.clone(),
                    // runtime,
                    store,
                })
            }
            Module::Jitted {
                original_bytes,
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
                    return Err(RuntimeError::ModuleRequestedTooMuchMemory);
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
                // Ok(Instance::Compiled {
                //     original_bytes: original_bytes.clone(),
                //     // compiled_module: compiled_module.clone(),
                //     instance,
                //     store,
                //     precompile_time: precompile_time.clone(),
                //     runtime,
                // })
                todo!("jit not available")
            }
            Module::Singlepass {
                original_bytes,
                wasmer_module,
                mut store,
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
                    return Err(RuntimeError::ModuleRequestedTooMuchMemory);
                }
                let preprocess_dur = timestamp.elapsed();

                let memory_pages = self.wasm_config().max_memory;

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
                    // let mut store = self.engine.as_wasmer();

                    let memory = wasmer::Memory::new(&mut store, imported_memory.clone())
                        .expect("should create wasmer memory");
                    Some(memory)
                } else {
                    None
                };

                // let memory_obj = memory;
                // // import_object.mem

                let wasmer_env = WasmerEnv {
                    host: Arc::new(RwLock::new(runtime)),
                    memory: imported_memory.clone(),
                };

                // let mut engine = self.engine.as_wasmer().expect("valid config");
                // let store = self.engine.as_wasmer_mut().expect("valid config");

                let function_env = FunctionEnv::new(&mut store, wasmer_env);

                let mut import_object =
                    wasmer_backend::make_wasmer_imports("env", &mut store, &function_env);

                if let Some(imported_memory) = &imported_memory {
                    import_object.define(
                        "env",
                        "memory",
                        wasmer::Extern::from(imported_memory.clone()),
                    );
                }

                let instance = wasmer::Instance::new(
                    &mut store.as_store_mut(),
                    &wasmer_module,
                    &import_object,
                )?;

                // function_env.as_mut(&mut wasmer_store.as_store_mut(), )
                // instance.
                match instance.exports.get_memory("memory") {
                    Ok(exported_memory) => {
                        // dbg!(&exported_memory);
                        function_env.as_mut(&mut store.as_store_mut()).memory =
                            Some(exported_memory.clone());
                    }
                    Err(error) => {
                        // if imported_memory.is_none() {
                        //     // panic!("Instance does not import/export memory which we don't
                        // currently allow {:?}", error); }
                    }
                }

                Ok(Instance::Singlepass {
                    original_bytes: original_bytes.clone(),
                    // wasmer_module: wasmer_module,
                    instance,
                    // runtime,
                    store,
                    timestamp,
                    preprocess_dur,
                })
            }
        }
    }

    fn make_cache_key(&self, original_bytes: Bytes) -> CacheKey {
        CacheKey(wasmer::Triple::host(), self.wasm_config, original_bytes)
    }

    fn cache_get(
        &self,
        correlation_id: Option<&CorrelationId>,
        original_bytes: Bytes,
    ) -> Option<Bytes> {
        cache_get(correlation_id, &self.make_cache_key(original_bytes))
    }

    fn cache_set(
        &self,
        correlation_id: Option<&CorrelationId>,
        clone: Bytes,
        serialized_bytes: Bytes,
    ) {
        cache_set(correlation_id, self.make_cache_key(clone), serialized_bytes)
    }

    /// Creates module specific for execution mode.
    pub fn module_from_bytes(
        &self,
        correlation_id: Option<CorrelationId>,
        wasm_bytes: Bytes,
    ) -> Result<Module, PreprocessingError> {
        let start = Instant::now();

        let module = match self.execution_mode {
            ExecutionMode::Interpreted => {
                let engine = WasmiEngine::new();
                // let module = deserialize_interpreted(&wasm_bytes)?;

                // let module = instrument_module(module, &self.wasm_config)?;
                let module = wasmi::Module::new(&engine, &wasm_bytes[..]).map_err(|error| {
                    PreprocessingError::Deserialize(format!(
                        "from_parity_wasm_module: {}",
                        error.to_string()
                    ))
                })?;

                Module::Interpreted {
                    original_bytes: wasm_bytes,
                    timestamp: start,
                    module,
                    engine,
                }
            }
            ExecutionMode::Compiled { cache_artifacts } => {
                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(correlation_id.as_ref(), wasm_bytes.clone())
                    {
                        if let Some(correlation_id) = correlation_id.as_ref() {
                            correlation_id.record_property(Property::VMCacheHit {
                                original_bytes: wasm_bytes.clone(),
                            });
                        }
                        let wasmtime_module = self
                            .deserialize_compiled(&precompiled_bytes)
                            .expect("Should deserialize");
                        return Ok(Module::Compiled {
                            original_bytes: wasm_bytes,

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
                if let Some(correlation_id) = correlation_id.as_ref() {
                    correlation_id.record_property(Property::VMCacheMiss {
                        original_bytes: wasm_bytes.clone(),
                    });
                }
                let module = wasmtime::Module::new(
                    &self.engine.as_wasmtime().expect("valid config"),
                    &wasm_bytes,
                )
                .unwrap();

                if cache_artifacts {
                    let serialized_bytes = module
                        .serialize()
                        .expect("should serialize wasmtime module");
                    self.cache_set(
                        correlation_id.as_ref(),
                        wasm_bytes.clone(),
                        serialized_bytes.into(),
                    );
                }

                Module::Compiled {
                    original_bytes: wasm_bytes,

                    precompile_time: None,
                    wasmtime_module: module,
                    // compiled_artifact: precompiled_bytes.to_owned(),
                }
            }
            ExecutionMode::JustInTime => {
                let start = Instant::now();
                let module = wasmtime::Module::new(
                    &self.engine.as_wasmtime().expect("valid config"),
                    wasm_bytes.clone(),
                )
                .unwrap();
                let stop = start.elapsed();
                Module::Jitted {
                    original_bytes: wasm_bytes,

                    precompile_time: Some(stop),
                    module,
                }
            }
            ExecutionMode::Wasmer(Wasmer {
                backend,
                cache_artifacts,
                instrument,
            }) => {
                // let hash_digest = base16::encode_lower(&blake2b(&wasm_bytes));
                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(correlation_id.as_ref(), wasm_bytes.clone())
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

                            wasmer_module,
                            store,
                            timestamp: start,
                        });
                    }
                }

                // let wasmer = self.execution_mode.as_wasmer().expect("valid config");
                let engine = make_wasmer_backend(backend, instrument, self.wasm_config);
                let wasmer_store = wasmer::Store::new(engine);

                let wasmer_module = wasmer::Module::from_binary(&wasmer_store, &wasm_bytes)
                    .expect("should create wasmer module");

                if cache_artifacts {
                    if let Some(correlation_id) = correlation_id.as_ref() {
                        correlation_id.record_property(Property::VMCacheMiss {
                            original_bytes: wasm_bytes.clone(),
                        });
                    }
                    let serialized_bytes = wasmer_module.serialize()?;
                    self.cache_set(
                        correlation_id.as_ref(),
                        wasm_bytes.clone(),
                        serialized_bytes,
                    );
                }

                // TODO: Cache key should be the one from StoredValue::ContractWasm which is random,
                // there's probably collision and somehow unmodified module is cached?

                Module::Singlepass {
                    original_bytes: wasm_bytes,

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
        let engine = match wasm_config.execution_mode {
            ExecutionMode::Interpreted => {
                // let config = wasmi::Config::default();
                // Can't reuse engine due to locking issue inside wasmi
                Engine::Wasmi
            }
            ExecutionMode::Compiled { .. } => Engine::Wasmtime(new_compiled_engine(&wasm_config)),
            ExecutionMode::JustInTime => todo!("jit temporarily unavailable"),
            ExecutionMode::Wasmer(Wasmer {
                backend,
                cache_artifacts,
                instrument,
            }) => Engine::Wasmer,
        };
        Self {
            wasm_config,
            execution_mode: wasm_config.execution_mode,
            engine: Arc::new(engine),
        }
    }
    // fn precompile(
    //     &self,
    //     original_bytes: Bytes,
    //     bytes: Bytes,
    //     cache_artifacts: bool,
    // ) -> Result<(Bytes, Option<Duration>), RuntimeError> {
    //     // if cache_artifacts {
    //     //     if let Some(precompiled) = self.cache_get(correlation_id.as_ref(),
    // original_bytes.clone()) {     //         let deserialized_module =
    //     //             unsafe { wasmtime::Module::deserialize(&self.compiled_engine,
    // &precompiled) }     //                 .expect("should deserialize wasmtime module");
    //     //         return Ok(());
    //     //     }
    //     // }
    //     // todo!("precompile; disabled because of trait issues colliding with wasmer");
    //     // let mut cache = GLOBAL_CACHE.lock().unwrap();
    //     // let mut cache = cache.borrow_mut();
    //     // let mut cache = GLOBAL_CACHE.lborrow_mut();
    //     // match self.cache_get(correlation_id.as_ref(), bytes)

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
        mut correlation_id: Option<CorrelationId>,
        module_bytes: &Bytes,
    ) -> Result<Module, PreprocessingError> {
        let start = Instant::now();

        // NOTE: Consider using `bytes::Bytes` instead of `bytesrepr::Bytes` as its cheaper

        if let Some(correlation_id) = correlation_id.as_mut() {
            correlation_id.record_property(Property::Preprocess {
                original_bytes: module_bytes.clone(),
            });
        };

        if self.execution_mode.is_singlepass() && self.execution_mode.is_using_cache() {
            if let Some(precompiled_bytes) =
                self.cache_get(correlation_id.as_ref(), module_bytes.clone())
            {
                // We have the artifact, and we can use headless mode to just execute binary
                // artifact
                let headless_engine = wasmer::Engine::headless();
                let store = wasmer::Store::new(headless_engine);
                let wasmer_module =
                    unsafe { wasmer::Module::deserialize(&store, precompiled_bytes.clone()) }?;

                return Ok(Module::Singlepass {
                    original_bytes: module_bytes.clone(),
                    wasmer_module,
                    store: store,
                    timestamp: start,
                });
            }
        }
        match self.execution_mode {
            ExecutionMode::Interpreted => {
                let engine = WasmiEngine::new();
                let deserialized_module = deserialize_interpreted(&module_bytes)?;

                let instrument_start = Instant::now();
                let module = instrument_module(deserialized_module, &self.wasm_config)?;
                let serialized_module = serialize_interpreted(module)?;
                println!("instrument {:?}", instrument_start.elapsed());
                // let serialized = serialize_interpreted(module)
                //                     .map_err(|error| RuntimeError::Other(error.to_string()))?;
                let parse_start = Instant::now();
                let module =
                    wasmi::Module::new(&engine, &serialized_module[..]).map_err(|error| {
                        PreprocessingError::Deserialize(format!(
                            "from_parity_wasm_module: {}",
                            error.to_string()
                        ))
                    })?;
                println!("wasmi from_parity_wasm_module {:?}", parse_start.elapsed());
                Ok(Module::Interpreted {
                    original_bytes: module_bytes.clone(),
                    timestamp: start,
                    module,
                    engine,
                })
            }
            ExecutionMode::Compiled { cache_artifacts } => {
                // TODO: Gas injected module is used here but we might want to use `module` instead
                // with other preprocessing done.
                // let preprocessed_wasm_bytes = Bytes::from(
                //     parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes"),
                // );
                // let preprocessed_wasm_b
                let wasmtime_engine = self.engine.as_wasmtime().expect("valid config");

                if cache_artifacts {
                    if let Some(precompiled_bytes) =
                        self.cache_get(correlation_id.as_ref(), module_bytes.clone())
                    {
                        if let Some(correlation_id) = correlation_id.as_ref() {
                            correlation_id.record_property(Property::VMCacheHit {
                                original_bytes: module_bytes.clone(),
                            });
                        }
                        let wasmtime_module = unsafe {
                            wasmtime::Module::deserialize(&wasmtime_engine, &precompiled_bytes)
                        }
                        .expect("should deserialize wasmtime module");
                        return Ok(Module::Compiled {
                            original_bytes: module_bytes.clone(),
                            precompile_time: None,
                            wasmtime_module,
                        });
                    }
                }

                let wasmtime_module = wasmtime::Module::new(
                    &self.engine.as_wasmtime().expect("valid config"),
                    &module_bytes,
                )
                .map_err(|error| PreprocessingError::Precompile(error.to_string()))?;

                if cache_artifacts {
                    if let Some(correlation_id) = correlation_id.as_ref() {
                        correlation_id.record_property(Property::VMCacheMiss {
                            original_bytes: module_bytes.clone(),
                        });
                    }
                    let serialized_bytes = wasmtime::Module::serialize(&wasmtime_module).unwrap();
                    self.cache_set(
                        correlation_id.as_ref(),
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
                    original_bytes: module_bytes.clone(),
                    precompile_time: None,
                    wasmtime_module,
                    // compiled_artifact: precompiled_bytes,
                })
            }
            ExecutionMode::JustInTime => {
                let start = Instant::now();

                // let preprocessed_wasm_bytes =
                //     parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes");
                let compiled_module = wasmtime::Module::new(
                    &self.engine.as_wasmtime().expect("valid config"),
                    &module_bytes,
                )
                .map_err(|e| PreprocessingError::Deserialize(e.to_string()))?;

                let stop = start.elapsed();

                Ok(Module::Jitted {
                    original_bytes: module_bytes.clone(),
                    precompile_time: Some(stop),
                    module: compiled_module,
                })
            }
            ExecutionMode::Wasmer(Wasmer {
                backend,
                cache_artifacts,
                instrument,
            }) => {
                // let preprocessed_wasm_bytes =
                //     parity_wasm::serialize(module.clone()).expect("preprocessed wasm to bytes");

                let start = Instant::now();

                // let mut store = wasmer::Store::new(self.engine.as_wasmer().expect("valid
                // config"));
                // let engine = self.engine.as_wasmer().expect("valid config");
                let engine = make_wasmer_backend(backend, instrument, self.wasm_config);
                let store = wasmer::Store::new(engine);
                // .read()
                // .unwrap();
                let original_bytes = module_bytes.clone();
                let module_bytes = match instrument {
                    InstrumentMode::None => module_bytes.clone(),
                    InstrumentMode::ParityWasm => {
                        // let instrument_Start = Instant::now();
                        let instrument_start = Instant::now();
                        let module = deserialize_interpreted(&module_bytes)?;
                        let instrumented = instrument_module(module, &self.wasm_config)?;
                        let instrumented_bytes = serialize_interpreted(instrumented)?;

                        // let module = instrument_module(deserialized_module, &self.wasm_config)?;
                        println!("wasmer instrument {:?}", instrument_start.elapsed());
                        Bytes::from(instrumented_bytes)
                    }
                    InstrumentMode::MeteringMiddleware => {
                        // Middleware should be already created
                        module_bytes.clone()

                        // module_bytes.clone()
                        // println!("")
                    }
                };
                let parse_start = Instant::now();
                let wasmer_module = wasmer::Module::from_binary(&store, &module_bytes)?;
                println!("wasmer from binary {:?}", parse_start.elapsed());

                if cache_artifacts {
                    if let Some(correlation_id) = correlation_id.as_ref() {
                        correlation_id.record_property(Property::VMCacheMiss {
                            original_bytes: module_bytes.clone(),
                        });
                    }
                    let serialized_bytes = wasmer_module.serialize()?;
                    self.cache_set(
                        correlation_id.as_ref(),
                        module_bytes.clone(),
                        serialized_bytes,
                    );
                }

                let stop = start.elapsed();

                Ok(Module::Singlepass {
                    original_bytes: original_bytes,
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
