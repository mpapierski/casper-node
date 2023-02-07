use wasmer::{
    AsStoreMut, Function, FunctionEnv, FunctionEnvMut, Imports, Memory, MemoryView, RuntimeError,
};

use crate::for_each_host_function;

use super::{host_interface::WasmHostInterface, FunctionContext};

pub(crate) struct WasmerEnv<H>
where
    H: WasmHostInterface + Send + Sync + 'static,
    H::Error: std::error::Error,
{
    pub(crate) host: H,
    pub(crate) memory: Option<Memory>,
}

pub(crate) struct WasmerAdapter<'a>(MemoryView<'a>);
impl<'a> WasmerAdapter<'a> {
    pub(crate) fn new(memory_view: MemoryView<'a>) -> Self {
        Self(memory_view)
    }
}

impl<'a> FunctionContext for WasmerAdapter<'a> {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, super::RuntimeError> {
        let mut vec = vec![0; size];
        self.0.read(offset as u64, &mut vec)?;
        Ok(vec)
    }

    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), super::RuntimeError> {
        self.0.write(offset as u64, data)?;
        Ok(())
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
                        let wasmer_memory = caller.data().memory.as_ref().cloned().unwrap();
                        let view = wasmer_memory.view(&caller);
                        let function_context = WasmerAdapter::new(view);

                        let res: visit_host_function!(@optional_ret $($ret)?) = match caller.data_mut().host.$name(function_context, $($($arg ),*)?) {
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
