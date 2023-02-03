use thiserror::Error;

use crate::shared::wasm_engine::FunctionContext;

use crate::for_each_host_function;

macro_rules! define_trait_methods {
    (@optional_ret) => { () };
    (@optional_ret $ret:tt) => { $ret };

    ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        $(
            $(#[$cfg])?
            fn $name(&mut self, _context: impl FunctionContext $($(,$arg: $argty)*)?) -> Result<define_trait_methods!(@optional_ret $($ret)?), Self::Error> {
                todo!()
            }
        )*
    };
}

pub(crate) trait WasmHostInterface {
    type Error: Send + Sync;

    for_each_host_function!(define_trait_methods);
}

#[cfg(test)]
pub(crate) struct HostStub;

#[cfg(test)]
#[derive(Error, Debug)]
#[error("stub error")]
pub(crate) struct StubError;

#[cfg(test)]
impl WasmHostInterface for HostStub {
    type Error = StubError;
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MockHost;
    impl WasmHostInterface for MockHost {
        type Error = ();
    }

    #[test]
    fn codegen() {}
}
