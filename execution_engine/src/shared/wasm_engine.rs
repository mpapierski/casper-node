//! Preprocessing of Wasm modules.
use casper_types::{
    account::AccountHash,
    api_error,
    bytesrepr::{self, Bytes, FromBytes, ToBytes},
    ApiError, CLValue, Gas, Key, ProtocolVersion, StoredValue, U512,
};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::{FromPrimitive, ToPrimitive};
use parity_wasm::elements::{self, MemorySection, Section};
use pwasm_utils::{self, stack_height};
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    error::Error,
    fmt::{self, Display, Formatter},
    rc::Rc,
};
use thiserror::Error;

const DEFAULT_GAS_MODULE_NAME: &str = "env";

use parity_wasm::elements::Module as WasmiModule;
use wasmi::{ImportsBuilder, MemoryRef, ModuleInstance, ModuleRef};
use wasmtime::{
    AsContextMut, Caller, Extern, ExternType, InstanceAllocationStrategy, Memory, MemoryType, Trap,
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
    // Compiled(Vec<u8>), AOT
    Compiled(wasmtime::Module),
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
// }

#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error(transparent)]
    WasmiError(#[from] wasmi::Error),
    #[error(transparent)]
    WasmtimeError(#[from] wasmtime::Trap),
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

// pub enum MemoryAdapter<'a> {
//     Interpreted(MemoryRef),
//     Compiled(&'a Cell<[u8]>),
// }

// impl<'a> MemoryAdapter<'a> {
//     pub fn set(&mut self, offset: u32, bytes: &[u8]) -> Result<(), RuntimeError> {
//         match self {
//             MemoryAdapter::Interpreted(memory_ref) => memory_ref.set(offset,
// bytes).map_err(RuntimeError::WasmiError)?,             MemoryAdapter::Compiled(_) => {
//                 todo!("compiled");
//             }
//         }
//         Ok(())
//     }
//     pub fn get(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError> {
//         match self {
//             MemoryAdapter::Interpreted(memory_ref) => Ok(memory_ref.get(offset, size)?),
//             MemoryAdapter::Compiled(_) => todo!(),
//         }
//     }
// }

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
                            // println!("Got {} from WebAssembly", param);
                            // println!("my host state is: {}", caller.data());
                            // todo!("revert {}", param);
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
                            let runtime = caller.data_mut();

                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();
                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.new_uref,
                                [uref_ptr, value_ptr, value_size],
                            )?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };

                            let mut buffer = vec![0; value_size as usize];
                            mem.read(&caller, value_ptr as usize, &mut buffer)
                                .map_err(|e| Trap::new("memory access"))?;
                            let cl_value =
                                bytesrepr::deserialize(buffer).map_err(execution::Error::from)?;

                            let uref = caller.data_mut().casper_new_uref(cl_value)?;

                            // let data = mem.data(&caller).get_mut(uref_ptr).ok_or_else(||
                            // Trap::new("bad ptr"))?;
                            // uref.into_bytes().map_err(execution::Error::BytesRepr)
                            // data.copy_from_slice(&?;
                            // mem.data
                            mem.write(&mut caller, uref_ptr as usize, &uref.into_bytes().unwrap())
                                .map_err(|_| Trap::new("memory access"))?;

                            // self.memory()
                            // .set(uref_ptr, &uref.into_bytes().map_err(Error::BytesRepr)?)
                            // .map_err(|e| Error::Interpreter(e.into()).into())

                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_create_purse",
                        |mut caller: Caller<'_, &mut Runtime<R>>, dest_ptr: u32, dest_size: u32| {
                            let runtime = caller.data_mut();

                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();
                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.create_purse,
                                [dest_ptr, dest_size],
                            )?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };

                            // let mut buffer = vec![0; value_size as usize];
                            // mem.read(&caller, value_ptr as usize, &mut buffer).map_err(|e|
                            // Trap::new("memory access"))?;
                            // let cl_value =
                            // bytesrepr::deserialize(buffer).map_err(execution::Error::from)?;

                            let uref = caller.data_mut().casper_create_purse()?;

                            // let data = mem.data(&caller).get_mut(uref_ptr).ok_or_else(||
                            // Trap::new("bad ptr"))?;
                            // uref.into_bytes().map_err(execution::Error::BytesRepr)
                            // data.copy_from_slice(&?;
                            // mem.data
                            mem.write(&mut caller, dest_ptr as usize, &uref.into_bytes().unwrap())
                                .map_err(|_| Trap::new("memory access"))?;

                            // self.memory()
                            // .set(uref_ptr, &uref.into_bytes().map_err(Error::BytesRepr)?)
                            // .map_err(|e| Error::Interpreter(e.into()).into())

                            Ok(())
                        },
                    )
                    .unwrap();

                // et (dest_ptr, dest_size) = Args::parse(args)?;
                // self.charge_host_function_call(
                //     &host_function_costs.create_purse,
                //     [dest_ptr, dest_size],
                // )?;
                // let purse = self.create_purse()?;
                // let purse_bytes = purse.into_bytes().map_err(Error::BytesRepr)?;
                // assert_eq!(dest_size, purse_bytes.len() as u32);
                // self.memory()
                //     .set(dest_ptr, &purse_bytes)
                //     .map_err(|e| Error::Interpreter(e.into()))?;
                //     // Ok(Some(RuntimeValue::I32(0)))

                //     Ok(())
                // }).unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_write",
                        |mut caller: Caller<'_, &mut Runtime<R>>,
                         key_ptr,
                         key_size,
                         value_ptr,
                         value_size| {
                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();

                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.write,
                                [key_ptr, key_size, value_ptr, value_size],
                            )?;
                            // let key = self.key_from_mem(key_ptr, key_size)?;
                            // let cl_value = self.cl_value_from_mem(value_ptr, value_size)?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };

                            let key = {
                                let mut buffer = vec![0; key_size as usize];
                                mem.read(&caller, key_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                let key: Key = bytesrepr::deserialize(buffer)
                                    .map_err(execution::Error::from)?;
                                key
                            };

                            let cl_value = {
                                let mut buffer = vec![0; value_size as usize];
                                mem.read(&caller, value_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                let key: CLValue = bytesrepr::deserialize(buffer)
                                    .map_err(execution::Error::from)?;
                                key
                            };

                            let mut runtime = caller.data_mut();
                            runtime.casper_write(key, cl_value)?;

                            Ok(())
                        },
                    )
                    .unwrap();

                linker
                    .func_wrap(
                        "env",
                        "casper_get_main_purse",
                        |mut caller: Caller<'_, &mut Runtime<R>>, dest_ptr: u32| {
                            let purse = caller.data_mut().casper_get_main_purse()?;

                            let purse_bytes = purse.into_bytes().map_err(execution::Error::from)?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();

                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.get_main_purse,
                                [dest_ptr],
                            )?;
                            let uref = caller.data_mut().casper_get_main_purse()?;

                            // let data = mem.data(&caller).get_mut(uref_ptr).ok_or_else(||
                            // Trap::new("bad ptr"))?;
                            // uref.into_bytes().map_err(execution::Error::BytesRepr)
                            // data.copy_from_slice(&?;
                            // mem.data
                            mem.write(&mut caller, dest_ptr as usize, &uref.into_bytes().unwrap())
                                .map_err(|_| Trap::new("memory access"))?;

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
                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();

                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.get_named_arg_size,
                                [name_ptr, name_size, size_ptr],
                            )?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };

                            let name = {
                                let mut buffer = vec![0; name_size as usize];
                                mem.read(&caller, name_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                // let name_bytes: Vec<u8> =
                                // bytesrepr::deserialize(buffer).map_err(execution::Error::from)?;
                                let name = String::from_utf8_lossy(buffer.as_slice()).to_string();
                                name
                            };

                            let res = match caller.data_mut().casper_get_named_arg_size(name) {
                                Ok(arg_size) => {
                                    let bytes = arg_size.to_le_bytes();
                                    mem.write(&mut caller, size_ptr as usize, &bytes)
                                        .map_err(|_| Trap::new("memory access"))?;
                                    Ok(())
                                }
                                Err(api_error) => Err(api_error),
                            };

                            // Ok(Some(RuntimeValue::I32(api_error::i32_from(res))))\
                            Ok(api_error::i32_from(res))
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
                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();

                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.get_named_arg,
                                [name_ptr, name_size, dest_ptr, dest_size],
                            )?;

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };

                            let name = {
                                let mut buffer = vec![0; name_size as usize];
                                mem.read(&caller, name_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                // let name_bytes: Vec<u8> =
                                // bytesrepr::deserialize(buffer).map_err(execution::Error::from)?;
                                let name = String::from_utf8_lossy(buffer.as_slice()).to_string();
                                name
                            };

                            let res = match caller.data_mut().casper_get_named_arg(&name) {
                                Ok(arg) => {
                                    // let bytes = arg_size.to_le_bytes();

                                    if arg.inner_bytes().len() > dest_size as usize {
                                        return Err(execution::Error::Revert(
                                            ApiError::OutOfMemory,
                                        )
                                        .into());
                                    }

                                    // let memory = self.instance.interpreted_memory();
                                    // if let Err(e) = memory.set(dest_ptr, )
                                    // {
                                    //     return Err(Error::Interpreter(e.into()).into());
                                    // }
                                    mem.write(
                                        &mut caller,
                                        dest_ptr as usize,
                                        &arg.inner_bytes()[..dest_size as usize],
                                    )
                                    .map_err(|_| Trap::new("memory access"))?;
                                    Ok(())
                                }
                                Err(api_error) => Err(api_error),
                            };

                            // Ok(Some(RuntimeValue::I32(api_error::i32_from(res))))\
                            Ok(api_error::i32_from(res))
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
                            let host_function_costs = caller
                                .data_mut()
                                .wasm_engine()
                                .wasm_config()
                                .take_host_function_costs();

                            let mem = match caller.get_export("memory") {
                                Some(Extern::Memory(mem)) => mem,
                                _ => return Err(Trap::new("failed to find host memory")),
                            };
                            let account_hash: AccountHash = {
                                let mut buffer = vec![0; key_size as usize];
                                mem.read(&caller, key_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                bytesrepr::deserialize(buffer).map_err(execution::Error::from)?
                            };

                            let amount: U512 = {
                                let mut buffer = vec![0; amount_size as usize];
                                mem.read(&caller, amount_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                bytesrepr::deserialize(buffer).map_err(execution::Error::from)?
                            };

                            let id: Option<u64> = {
                                let mut buffer = vec![0; id_size as usize];
                                mem.read(&caller, id_ptr as usize, &mut buffer)
                                    .map_err(|e| Trap::new("memory access"))?;
                                bytesrepr::deserialize(buffer).map_err(execution::Error::from)?
                            };
                            // let () =
                            // Args::parse(args)?;
                            caller.data_mut().charge_host_function_call(
                                &host_function_costs.transfer_to_account,
                                [
                                    key_ptr,
                                    key_size,
                                    amount_ptr,
                                    amount_size,
                                    id_ptr,
                                    id_size,
                                    result_ptr,
                                ],
                            )?;

                            let ret = match caller.data_mut().casper_transfer_to_account(
                                account_hash,
                                amount,
                                id,
                            )? {
                                Ok(transferred_to) => {
                                    let result_value: u32 = transferred_to as u32;
                                    let result_value_bytes = result_value.to_le_bytes();
                                    // let bytes = arg_size.to_le_bytes();
                                    mem.write(
                                        &mut caller,
                                        result_ptr as usize,
                                        &result_value_bytes,
                                    )
                                    .map_err(|_| Trap::new("memory access"))?;
                                    Ok(())
                                }
                                Err(api_error) => Err(api_error),
                            };
                            Ok(api_error::i32_from(ret))
                            // Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
                            // todo!("transfer")
                            // Ok(0)
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

/// Wasm preprocessor.
pub struct WasmEngine {
    wasm_config: WasmConfig,
    execution_mode: ExecutionMode,
    compiled_engine: wasmtime::Engine,
}

fn new_compiled_engine(wasm_config: &WasmConfig) -> wasmtime::Engine {
    let mut config = wasmtime::Config::new();
    config.async_support(false);
    config
        .max_wasm_stack(wasm_config.max_stack_height as usize)
        .expect("should set max stack");
    // TODO: Tweak more
    wasmtime::Engine::new(&config).expect("should create new engine")
}

impl WasmEngine {
    /// Creates a new instance of the preprocessor.
    pub fn new(wasm_config: WasmConfig) -> Self {
        Self {
            wasm_config,
            execution_mode: wasm_config.execution_mode,
            compiled_engine: new_compiled_engine(&wasm_config),
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
                // let precompiled_bytes =
                // self.compiled_engine.precompile_module(&preprocessed_wasm_bytes).expect("should
                // preprocess"); jit?
                let wasmtime_module =
                    wasmtime::Module::new(&self.compiled_engine, &preprocessed_wasm_bytes)
                        .expect("should process");
                Ok(Module::Compiled(wasmtime_module))

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
                // let precompiled_bytes =
                // self.compiled_engine.precompile_module(&wasm_bytes).expect("should preprocess");
                let module = wasmtime::Module::new(&self.compiled_engine, wasm_bytes).unwrap();
                Module::Compiled(module)
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

                // let compiled_module = unsafe {
                // wasmtime::Module::deserialize(&self.compiled_engine(),
                // &compiled_module).expect("should load precompiled module") };
                // todo!("compiled mode")
                // let mut store = wasmtime::Store::new(&wasm_engine.compiled_engine(), ());
                // let instance = wasmtime::Instance::new(&mut store, &compiled_module,
                // &[]).expect("should create compiled module");
                Ok(Instance::Compiled(compiled_module))
            }
        }
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
