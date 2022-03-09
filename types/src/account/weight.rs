use alloc::vec::Vec;

use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "datasize")]
use datasize::DataSize;
use serde::{Deserialize, Serialize};

use crate::{CLType, CLTyped};

/// The weight attributed to a given [`AccountHash`](super::AccountHash) in an account's associated
/// keys.
#[derive(
    PartialOrd,
    Ord,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
pub struct Weight(u8);

impl Weight {
    /// Constructs a new `Weight`.
    pub const fn new(weight: u8) -> Weight {
        Weight(weight)
    }

    /// Returns the value of `self` as a `u8`.
    pub fn value(self) -> u8 {
        self.0
    }
}

impl CLTyped for Weight {
    fn cl_type() -> CLType {
        CLType::U8
    }
}
