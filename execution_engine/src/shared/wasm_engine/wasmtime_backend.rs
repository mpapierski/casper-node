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

pub struct WasmtimeEnv<H>
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

pub(crate) fn make_linker_object<H>(env_name: &str, engine: &Engine) -> Linker<WasmtimeEnv<H>>
where
    H: WasmHostInterface,
    H::Error: std::error::Error + Send + Sync + 'static,
{
    let mut linker = Linker::new(engine);

    macro_rules! visit_host_function {
        (@optional_ret) => { () };
        (@optional_ret $ret:tt) => { $ret };

        ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {{
            $(
                $(#[$cfg])?
                {
                linker
                    .func_wrap(
                        env_name,
                        stringify!($name),
                        |
                            mut caller: Caller<WasmtimeEnv<H>>,
                            $($($arg: $argty),*)?
                        | {
                            let (function_context, host) =
                                caller_adapter_and_runtime(&mut caller);

                                let ret: visit_host_function!(@optional_ret $($ret)?) = match host.$name(
                                    function_context,
                                    $($($arg ),*)?
                                ) {
                                    Ok(result) => result,
                                    Err(error) => {
                                        return Err(make_wasmtime_trap(error))
                                    }
                                };

                                Ok(ret)
                        },
                    )
                    .unwrap();
                }
            )*
        }};
    }

    for_each_host_function!(visit_host_function);

    linker
}
