use alloc::vec::Vec;
use borsh::{BorshSerialize, BorshDeserialize};

/// A newtype wrapping a [`u64`] which represents the block time.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct BlockTime(u64);

impl BlockTime {
    /// Constructs a `BlockTime`.
    pub fn new(value: u64) -> Self {
        BlockTime(value)
    }

    /// Saturating integer subtraction. Computes `self - other`, saturating at `0` instead of
    /// overflowing.
    #[must_use]
    pub fn saturating_sub(self, other: BlockTime) -> Self {
        BlockTime(self.0.saturating_sub(other.0))
    }
}

impl From<BlockTime> for u64 {
    fn from(blocktime: BlockTime) -> Self {
        blocktime.0
    }
}
