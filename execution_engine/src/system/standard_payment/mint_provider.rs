use casper_types::{ApiError, URef, U512};

use crate::shared::wasm_engine::FunctionContext;

/// Provides an access to mint.
pub trait MintProvider {
    /// Transfer `amount` of tokens from `source` purse to a `target` purse.
    fn transfer_purse_to_purse(
        &mut self,
        context: &mut impl FunctionContext,
        source: URef,
        target: URef,
        amount: U512,
    ) -> Result<(), ApiError>;
}
