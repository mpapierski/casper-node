//! Wasm helpers.
use parity_wasm::builder;

use casper_types::contracts::DEFAULT_ENTRY_POINT_NAME;

use crate::shared::wasm_engine::{PreprocessingError, WasmEngine};

use super::wasm_engine::Module;

/// Creates minimal session code that does nothing
pub fn do_nothing_bytes() -> Vec<u8> {
    let module = builder::module()
        .function()
        // A signature with 0 params and no return type
        .signature()
        .build()
        .body()
        .build()
        .build()
        // Export above function
        .export()
        .field(DEFAULT_ENTRY_POINT_NAME)
        .build()
        // Memory section is mandatory
        .memory()
        .build()
        .build();
    parity_wasm::serialize(module).expect("should serialize")
}

/// Creates a module which is a valid Wasm but does nothing.
pub fn do_nothing_module(preprocessor: &WasmEngine) -> Result<Module, PreprocessingError> {
    let do_nothing_bytes = do_nothing_bytes();
    preprocessor.preprocess(&do_nothing_bytes)
}
