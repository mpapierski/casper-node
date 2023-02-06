use std::{
    borrow::{BorrowMut, Cow},
    cell::RefCell,
    collections::HashMap,
    sync::Arc,
};

use once_cell::sync::Lazy;
use thiserror::Error;
use wasmi::{
    core::{HostError, Trap},
    errors::LinkerError,
    AsContext, AsContextMut, Caller, Config, Engine, Error, Func, FuncRef, Linker, Memory, Store,
};

use crate::for_each_host_function;

use super::{host_interface::WasmHostInterface, FunctionContext, RuntimeError};

pub struct WasmiEngine(Engine);

impl std::ops::Deref for WasmiEngine {
    type Target = Engine;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl WasmiEngine {
    pub(crate) fn new() -> Self {
        let mut config = Config::default();
        config.floats(true);
        let engine = Engine::new(&config);
        Self(engine)
    }
}

#[derive(Error, Debug)]
#[error("{}", source)]
pub(crate) struct IndirectHostError<E: std::error::Error + Send + Sync + 'static> {
    #[source]
    pub(crate) source: E,
}

impl<E: std::error::Error + Send + Sync + 'static> HostError for IndirectHostError<E> {}

pub(crate) fn make_wasmi_host_error<E: std::error::Error + Send + Sync + 'static>(
    error: E,
) -> impl HostError {
    IndirectHostError { source: error }
}

pub(crate) struct WasmiAdapter<'a> {
    data: &'a mut [u8],
}

impl<'a> FunctionContext for WasmiAdapter<'a> {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError> {
        let mut buffer = vec![0; size];

        let slice = self
            .data
            .get(offset as usize..)
            .and_then(|s| s.get(..buffer.len()))
            .ok_or_else(|| RuntimeError::WasmiTrap(Arc::new(Trap::new("memory access"))))?;
        buffer.copy_from_slice(slice);
        Ok(buffer)
    }

    fn memory_write(&mut self, offset: u32, buffer: &[u8]) -> Result<(), RuntimeError> {
        self.data
            .get_mut(offset as usize..)
            .and_then(|s| s.get_mut(..buffer.len()))
            .ok_or_else(|| RuntimeError::WasmiTrap(Arc::new(Trap::new("memory access"))))?
            .copy_from_slice(buffer);
        Ok(())
    }
}

pub struct WasmiEnv<H: WasmHostInterface> {
    pub(crate) host: H,
    pub(crate) memory: Option<Memory>,
}

pub(crate) fn make_wasmi_linker<H>(
    env_name: &str,
    store: &mut impl AsContextMut<UserState = WasmiEnv<H>>,
) -> Result<Linker<WasmiEnv<H>>, LinkerError>
where
    H: WasmHostInterface,
    H::Error: std::error::Error + 'static,
{
    let mut linker = Linker::new();

    macro_rules! visit_host_function {
        (@optional_ret) => { () };
        (@optional_ret $ret:tt) => { $ret };

        ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {{
            $(
                $(#[$cfg])?
                {
                let func = Func::wrap(
                    store.as_context_mut(),
                    // env,
                    |
                        mut caller: Caller<'_, WasmiEnv<H>>,
                        $($($arg: $argty),*)?
                    | -> Result<visit_host_function!(@optional_ret $($ret)?), Trap> {
                        // let (data, mem) =
                        let wasmi_memory = caller.data().memory.as_ref().cloned().unwrap();
                        let (data, store) = wasmi_memory.data_and_store_mut(caller.as_context_mut());
                        // let
                        let function_context = WasmiAdapter { data: data };
                        // caller.data().host.
                        // let view = wasmer_memory.view(&caller);
                        // let function_context = WasmerAdapter::new(view);

                        let res: visit_host_function!(@optional_ret $($ret)?) = match store.host.$name(function_context, $($($arg ),*)?) {
                            Ok(result) => result,
                            Err(error) => {
                                let indirect = IndirectHostError { source: error };
                                return Err(indirect.into());
                            }
                        };
                        Ok(res)
                        // todo!("todo {}", stringify!($name));
                    }
                );

                linker.define(env_name, stringify!($name), func)?;
                }
            )*
        }};
    }

    for_each_host_function!(visit_host_function);

    Ok(linker)
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     // #[test]
//     // fn cfg_attribute_is_respected_in_macro() {
//     //     if cfg!(feature = "test-support") {
//     //         assert!(NAME_LOOKUP_TABLE.contains_key(&"casper_print"));
//     //     } else {
//     //         assert!(!NAME_LOOKUP_TABLE.contains_key(&"casper_print"));
//     //     }
//     // }
// }
