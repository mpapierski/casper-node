use casper_types::{ApiError, URef};

use crate::shared::wasm_engine::FunctionContext;

/// Provider of handle payment functionality.
pub trait HandlePaymentProvider {
    /// Get payment purse for given deploy.
    fn get_payment_purse(&mut self, context: &mut impl FunctionContext) -> Result<URef, ApiError>;
}
