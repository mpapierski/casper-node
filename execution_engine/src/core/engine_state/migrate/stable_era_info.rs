//! Implements a migration for copying the current era info to a stable key.
use super::*;
use std::borrow::BorrowMut;

use crate::{
    core::{execution, tracking_copy::TrackingCopy},
    shared::newtypes::CorrelationId,
    storage::global_state::{CommitProvider, StateProvider},
};
use casper_types::{EraId, Key};

/// Errors that can occur while purging era info objects from global state.
#[derive(Clone, thiserror::Error, Debug)]
#[non_exhaustive]
pub enum StableKeyError {
    /// Execution Engine error.
    #[error("exec error: {0}")]
    Exec(execution::Error),

    /// Unable to retrieve last era info.
    #[error("unable to retrieve last era info")]
    UnableToRetrieveLastEraInfo(execution::Error),
    /// Root not found.
    #[error("root not found")]
    RootNotFound,

    /// Key does not exist.
    #[error("key does not exist")]
    KeyDoesNotExist,
}

/// Result of writing era info to a stable key.
#[derive(Debug, Clone)]
pub struct WroteStableKey {
    /// Post state hash.
    pub post_state_hash: Digest,
}

/// Write era info currently at era_id(number) key to stable key.
pub fn write_era_info_summary_to_stable_key<S>(
    state: &S,
    correlation_id: CorrelationId,
    state_root_hash: Digest,
    era_id: EraId,
) -> Result<WroteStableKey, StableKeyError>
where
    S: StateProvider + CommitProvider,
    S::Error: Into<execution::Error>,
{
    let mut tracking_copy = match state
        .checkout(state_root_hash)
        .map_err(|error| StableKeyError::Exec(error.into()))?
    {
        Some(tracking_copy) => TrackingCopy::new(tracking_copy),
        None => return Err(StableKeyError::RootNotFound),
    };

    let last_era_info = tracking_copy
        .borrow_mut()
        .get(correlation_id, &Key::EraInfo(era_id))
        .map_err(|error| StableKeyError::UnableToRetrieveLastEraInfo(error.into()))?
        .ok_or(StableKeyError::KeyDoesNotExist)?;

    tracking_copy.force_write(Key::EraSummary, last_era_info);

    let new_state_root_hash = state
        .commit(
            correlation_id,
            state_root_hash,
            tracking_copy.effect().transforms,
        )
        .map_err(|error| StableKeyError::Exec(error.into()))?;

    Ok(WroteStableKey {
        post_state_hash: new_state_root_hash,
    })
}
