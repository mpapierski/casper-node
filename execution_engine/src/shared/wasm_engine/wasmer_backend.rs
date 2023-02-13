pub mod metering;

use std::{
    marker::PhantomData,
    sync::{Arc, RwLock},
};

use wasmer::{
    AsStoreMut, AsStoreRef, CompilerConfig, Cranelift, Engine, Function, FunctionEnv,
    FunctionEnvMut, Imports, Memory, MemoryView, RuntimeError, StoreMut,
};
use wasmer_compiler_singlepass::Singlepass;

use crate::{
    for_each_host_function,
    shared::{
        wasm_config::WasmConfig,
        wasm_engine::{CraneliftOptLevel, InstrumentMode},
    },
};

use super::{host_interface::WasmHostInterface, FunctionContext, WasmerBackend};

pub(crate) fn make_wasmer_backend(
    backend: WasmerBackend,
    instrument: InstrumentMode,
    wasm_config: WasmConfig,
) -> Engine {
    match backend {
        WasmerBackend::Singlepass => {
            let mut compiler = Singlepass::new();
            if matches!(instrument, InstrumentMode::MeteringMiddleware) {
                compiler.push_middleware(metering::wasmer_metering(
                    u64::MAX,
                    wasm_config.opcode_costs,
                ));
            }
            compiler.into()
        }
        WasmerBackend::Cranelift { optimize } => {
            let mut compiler = Cranelift::new();
            match optimize {
                CraneliftOptLevel::None => {
                    compiler.opt_level(wasmer::CraneliftOptLevel::None);
                }
                CraneliftOptLevel::Speed => {
                    compiler.opt_level(wasmer::CraneliftOptLevel::Speed);
                }
                CraneliftOptLevel::SpeedAndSize => {
                    compiler.opt_level(wasmer::CraneliftOptLevel::SpeedAndSize);
                }
            }
            if matches!(instrument, InstrumentMode::MeteringMiddleware) {
                compiler.push_middleware(metering::wasmer_metering(
                    u64::MAX,
                    wasm_config.opcode_costs,
                ));
            }
            compiler.into()
        }
    }
}

pub(crate) struct WasmerEnv<H>
where
    H: WasmHostInterface + Send + Sync + 'static,
    H::Error: std::error::Error,
{
    pub(crate) host: Arc<RwLock<H>>,
    pub(crate) memory: Option<Memory>,
}

pub struct WasmerAdapter<'a> {
    pub(crate) view: MemoryView<'a>,
}

impl<'a> WasmerAdapter<'a> {
    fn new(view: MemoryView<'a>) -> Self {
        Self {
            view,
            // _marker: PhantomData,
        }
    }
}

impl<'a> FunctionContext for WasmerAdapter<'a> {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, super::RuntimeError> {
        // self.

        // let wasmer_memory = self.caller.data().memory.as_ref().unwrap();

        // let view = wasmer_memory.view(&self.caller);
        let mut vec = vec![0; size];
        self.view.read(offset as u64, &mut vec)?;
        Ok(vec)
        // todo!()
    }

    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), super::RuntimeError> {
        // let wasmer_memory = self.caller.data().memory.as_ref().unwrap();

        // let view = wasmer_memory.view(&self.caller);
        // let mut vec = vec![0; size];
        self.view.write(offset as u64, data)?;
        Ok(())
        // todo!()
    }
}

pub(crate) fn make_wasmer_imports<H>(
    env_name: &str,
    store: &mut impl AsStoreMut,
    env: &FunctionEnv<WasmerEnv<H>>,
) -> Imports
where
    H: WasmHostInterface + Send + Sync,
    H::Error: std::error::Error,
{
    let mut import_object = Imports::new();

    macro_rules! visit_host_function {
        (@optional_ret) => { () };
        (@optional_ret $ret:tt) => { $ret };

        ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {{
            $(
                $(#[$cfg])?
                {
                let func = Function::new_typed_with_env(
                    &mut store.as_store_mut(),
                    env,
                    |
                        mut caller: FunctionEnvMut<WasmerEnv<H>>,
                        $($($arg: $argty),*)?
                    | -> Result<visit_host_function!(@optional_ret $($ret)?), RuntimeError> {
                        // caller.
                        let view = caller.data().memory.as_ref().unwrap().view(&caller.as_store_ref());
                        let function_context = WasmerAdapter::new(view);

                        let mut host = caller.data().host.write().unwrap();

                        let res: visit_host_function!(@optional_ret $($ret)?) = match host.$name(function_context, $($($arg ),*)?) {
                            Ok(result) => result,
                            Err(error) => return Err(RuntimeError::user(error.into())),
                        };
                        Ok(res)
                    }
                );

                import_object.define(env_name, stringify!($name), func);
                }
            )*
        }};
    }

    for_each_host_function!(visit_host_function);

    import_object
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke_test() {}
}
