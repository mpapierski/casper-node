//! Preprocessing of Wasm modules.
use casper_types::{
    bytesrepr::{self, FromBytes, ToBytes},
    Key, StoredValue,
};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use parity_wasm::elements::{self, MemorySection, Section};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Display, Formatter},
    rc::Rc,
};
use thiserror::Error;

const DEFAULT_GAS_MODULE_NAME: &str = "env";

use parity_wasm::elements::Module as WasmiModule;
use wasmi::{MemoryRef, ModuleRef};

use crate::{
    core::{execution, runtime::Runtime},
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
#[derive(Debug, Clone, PartialEq)]
pub enum Module {
    Interpreted(WasmiModule),
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
    pub fn into_interpreted(self) -> Result<WasmiModule, PreprocessingError> {
        match self {
            Module::Interpreted(parity_wasm) => Ok(parity_wasm),
        }
    }
}

impl From<WasmiModule> for Module {
    fn from(wasmi_module: WasmiModule) -> Self {
        Self::Interpreted(wasmi_module)
    }
}

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    WasmiError(#[from] wasmi::Error),
}

impl RuntimeError {
    pub fn as_execution_error(&self) -> Option<&execution::Error> {
        match self {
            RuntimeError::WasmiError(wasmi_error) => wasmi_error
                .as_host_error()
                .and_then(|host_error| host_error.downcast_ref::<execution::Error>()),
        }
    }
}

// impl ToString for RuntimeError {
//     fn to_string(&self) -> String {
//         // NOTE: Using this is likely not portable across different Wasm implementations
//         match self {
//             RuntimeError::WasmiError(wasmi_error) => wasmi_error.into(),
//         }
//     }
// }

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
#[derive(Debug, Clone)]
pub enum Instance {
    Interpreted(ModuleRef, MemoryRef),
}

#[derive(Debug, Clone)]
pub struct InstanceRef(Rc<Instance>);

impl From<Instance> for InstanceRef {
    fn from(instance: Instance) -> Self {
        InstanceRef(Rc::new(instance))
    }
}

impl Instance {
    pub fn memory(&self) -> &MemoryRef {
        match self {
            Instance::Interpreted(_module_ref, memory_ref) => memory_ref,
        }
    }

    pub fn invoke_export<R>(
        &self,
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
        }
    }
}

/// An error emitted by the Wasm preprocessor.
#[derive(Debug, Clone, Error)]
pub enum PreprocessingError {
    /// Unable to deserialize Wasm bytes.
    Deserialize(String),
    /// Found opcodes forbidden by gas rules.
    OperationForbiddenByGasRules,
    /// Stack limiter was unable to instrument the binary.
    StackLimiter,
    /// Wasm bytes is missing memory section.
    MissingMemorySection,
}

impl From<elements::Error> for PreprocessingError {
    fn from(error: elements::Error) -> Self {
        PreprocessingError::Deserialize(error.to_string())
    }
}

impl Display for PreprocessingError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            PreprocessingError::Deserialize(error) => write!(f, "Deserialization error: {}", error),
            PreprocessingError::OperationForbiddenByGasRules => write!(f, "Encountered operation forbidden by gas rules. Consult instruction -> metering config map"),
            PreprocessingError::StackLimiter => write!(f, "Stack limiter error"),
            PreprocessingError::MissingMemorySection => write!(f, "Memory section should exist"),
        }
    }
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

/// Wasm preprocessor.
pub struct WasmEngine {
    wasm_config: WasmConfig,
    execution_mode: ExecutionMode,
}

impl WasmEngine {
    /// Creates a new instance of the preprocessor.
    pub fn new(wasm_config: WasmConfig) -> Self {
        Self {
            wasm_config,
            execution_mode: wasm_config.execution_mode,
        }
    }

    pub fn execution_mode(&self) -> &ExecutionMode {
        &self.execution_mode
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

        if memory_section(&module).is_none() {
            // `pwasm_utils::externalize_mem` expects a memory section to exist in the module, and
            // panics otherwise.
            return Err(PreprocessingError::MissingMemorySection);
        }

        let module = pwasm_utils::externalize_mem(module, None, self.wasm_config.max_memory);
        let module = pwasm_utils::inject_gas_counter(
            module,
            &self.wasm_config.opcode_costs().to_set(),
            DEFAULT_GAS_MODULE_NAME,
        )
        .map_err(|_| PreprocessingError::OperationForbiddenByGasRules)?;
        let module = stack_height::inject_limiter(module, self.wasm_config.max_stack_height)
            .map_err(|_| PreprocessingError::StackLimiter)?;

        match self.execution_mode {
            ExecutionMode::Interpreted => Ok(module.into()),
            ExecutionMode::Compiled => {
                // Convert modified pwasm module bytes into compiled module
                let _preprocessed_wasm_bytes =
                    parity_wasm::serialize(module).expect("preprocessed wasm to bytes");
                // TODO: preprocessed wasm bytes into compiled wasm modlue
                todo!("finish preprocessing compiled module")
            }
        }
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
            ExecutionMode::Compiled => todo!("compiled module from bytes"),
        };
        Ok(module)
    }
}
