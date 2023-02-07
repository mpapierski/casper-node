//! Definition of a Wasm host interface trait.
//!
//! This trait is used together with [`for_each_host_function`]
use crate::shared::wasm_engine::FunctionContext;

use crate::for_each_host_function;

macro_rules! define_trait_methods {
    (@optional_ret) => { () };
    (@optional_ret $ret:tt) => { $ret };

    ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        $(
            #[doc = stringify!($name)] // TODO: add support for doc strings in the macro. This is currently done just to satisfy the compiler.
            $(#[$cfg])?
            fn $name(&mut self, _context: impl FunctionContext $($(,$arg: $argty)*)?) -> Result<define_trait_methods!(@optional_ret $($ret)?), Self::Error>;
        )*
    };
}

/// Definition of a Wasm host interface.
///
/// This trait defines all the supported host functions.
pub trait WasmHostInterface {
    /// A host error that can be returned from a host function implementation.
    ///
    /// As currently implemented current design does not allow wasm traps to be raised from within
    /// host function i.e. you can't trap with unreachable. This is fine as long as all of the
    /// current host function implementations use host error to signal an error, and it stops
    /// execution.
    type Error: Send + Sync;

    for_each_host_function!(define_trait_methods);
}
