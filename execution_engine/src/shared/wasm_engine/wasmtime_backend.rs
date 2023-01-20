use thiserror::Error;
use wasmtime::{Caller, Engine, Linker, Trap};

use crate::for_each_host_function;

use super::{host_interface::WasmHostInterface, FunctionContext, RuntimeError};

/// Error indirection for wasmtime::Trap errors.
///
/// wasmtime::Trap does not expose the error itself that occurred during execution, only the source
/// of error. In our case the source would point at a variant of [`Error`]. We need the [`Error`]
/// itself so we have to wrap it before creating a Trap object.
#[derive(Error, Debug)]
#[error("{}", source)]
struct Indirect<E: std::error::Error + Send + Sync> {
    #[source]
    source: E,
}

fn make_wasmtime_trap<E: std::error::Error + Send + Sync + 'static>(error: E) -> Trap {
    let indirect = Indirect { source: error };
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(indirect);
    Trap::from(boxed)
}

/// Wasm caller object passed as an argument for each.
pub(crate) struct WasmtimeAdapter<'a> {
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

pub(crate) struct WasmtimeEnv<H>
where
    H: WasmHostInterface,
{
    pub(crate) host: H,
    // HACK: Runtime shouldn't know about this detail, it's here because of difficult lifetime
    // issues when dealing with wasmtime's Store object.
    pub(crate) wasmtime_memory: Option<wasmtime::Memory>,
}

fn caller_adapter_and_runtime<'b, 'c, 'd: 'c, H>(
    caller: &'c mut Caller<'d, WasmtimeEnv<H>>,
) -> (WasmtimeAdapter<'c>, &'c mut H)
where
    H: WasmHostInterface,
{
    let mem = caller.data().wasmtime_memory;
    let (data, runtime) = mem
        .expect("Memory should have been initialized.")
        .data_and_store_mut(caller);
    (WasmtimeAdapter { data }, &mut runtime.host)
}

pub(crate) fn make_linker_object<H>(
    env_name: &str,
    engine: &Engine,
    // store: &mut impl AsStoreMut,
    // env: &FunctionEnv<WasmerEnv<H>>,
) -> Linker<WasmtimeEnv<H>>
where
    H: WasmHostInterface,
    H::Error: std::error::Error + Send + Sync + 'static,
{
    let mut linker = Linker::new(engine);

    macro_rules! visit_host_function {
        ($( @$proposal:ident fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:tt)*) => {{
            $(
                // let func = Function::new_typed_with_env(
                //     &mut store.as_store_mut(),
                //     env,
                //     |
                //         mut caller: FunctionEnvMut<WasmerEnv<H>>,
                //         $($($arg: $argty),*)?
                //     | -> Result<$ret, RuntimeError> {
                //         let wasmer_memory = caller.data().memory.as_ref().cloned().unwrap();
                //         let view = wasmer_memory.view(&caller);
                //         let function_context = WasmerAdapter::new(view);

                //         match caller.data_mut().host.$name(function_context, $($($arg ),*)?) {
                //             Ok(result) => Ok(result),
                //             Err(error) => Err(RuntimeError::user(error.into())),
                //         }
                //     }
                // );

                // import_object.define(env_name, stringify!($name), func);

                linker
                    .func_wrap(
                        env_name,
                        stringify!($name),
                        |
                            mut caller: Caller<WasmtimeEnv<H>>,
                            $($($arg: $argty),*)?
                        | {
                            // todo!()
                            let (function_context, host) =
                                caller_adapter_and_runtime(&mut caller);

                                match host.$name(
                                    function_context,
                                    $($($arg ),*)?,
                                ) {
                                    Ok(result) => Ok(result),
                                    Err(error) => {
                                        return Err(make_wasmtime_trap(error))
                                    }
                                }

                            // let ret = runtime.read(
                            //     function_context,
                            //     key_ptr,
                            //     key_size,
                            //     output_size_ptr,
                            // )?;
                            // Ok(api_error::i32_from(ret))
                        },
                    )
                    .unwrap();

            )*
        }};
    }

    for_each_host_function!(visit_host_function);

    linker
}
