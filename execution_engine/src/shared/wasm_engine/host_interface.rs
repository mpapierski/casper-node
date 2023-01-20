use crate::{core::execution, for_each_host_function};

use super::FunctionContext;

macro_rules! define_trait_methods {
    ($( @$proposal:ident fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:tt)*) => {
        $(
            #[allow(unused)]
            fn $name(&mut self, _context: impl FunctionContext $($(,$arg: $argty)*)?) -> Result<$ret, Self::Error>;
            // define_trait_methods!(@define $proposal fn $name($($($arg: $argty),*)?) -> $ret);
        )*
    };

    // (@define core fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
    //     fn $name(&mut self, context: impl FunctionContext $($(,$arg: $argty)*)?) -> $ret;
    // };

    // (@define test fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
    //     #[cfg(feature = "test-support")]
    // };

    // (@define internal fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
    //     fn $name(&mut self, context: impl FunctionContext $($(,$arg: $argty)*)?) -> $ret;
    // };
}

pub(crate) trait WasmHostInterface {
    type Error: Send + Sync;

    for_each_host_function!(define_trait_methods);
}

// macro_rules! visit_internal_functions {
//     ($( @$proposal:ident fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident)*) => {
//         [$(
//             visit_internal_functions!(@define $proposal fn $name($($($arg: $argty),*)?) -> $ret);
//         )*]
//     };

//     (@define core fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
//     };

//     (@define test fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
//     };

//     (@define internal fn $name:ident $(( $($arg:ident: $argty:ty),* ))? -> $ret:ident) => {
//         stringify!($name),
//     };
// }
