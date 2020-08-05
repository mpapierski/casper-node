use alloc::{collections::BTreeMap, vec::Vec};

use crate::{
    bytesrepr::{self, ToBytes},
    CLType, CLTyped, PublicKey, URef, U512,
};
use bytesrepr::FromBytes;

#[derive(Copy, Clone)]
pub struct UnbondingPurse {
    pub purse: URef,
    pub origin: PublicKey,
    pub era_of_withdrawal: u64,
    pub amount: U512,
}

impl ToBytes for UnbondingPurse {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        let mut result = bytesrepr::allocate_buffer(self)?;
        result.extend(&self.purse.to_bytes()?);
        result.extend(&self.origin.to_bytes()?);
        result.extend(&self.era_of_withdrawal.to_bytes()?);
        result.extend(&self.amount.to_bytes()?);
        Ok(result)
    }
    fn serialized_length(&self) -> usize {
        self.purse.serialized_length()
            + self.origin.serialized_length()
            + self.era_of_withdrawal.serialized_length()
            + self.amount.serialized_length()
    }
}

impl FromBytes for UnbondingPurse {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (purse, bytes) = FromBytes::from_bytes(bytes)?;
        let (origin, bytes) = FromBytes::from_bytes(bytes)?;
        let (era_of_withdrawal, bytes) = FromBytes::from_bytes(bytes)?;
        let (amount, bytes) = FromBytes::from_bytes(bytes)?;
        Ok((
            UnbondingPurse {
                purse,
                origin,
                era_of_withdrawal,
                amount,
            },
            bytes,
        ))
    }
}

impl CLTyped for UnbondingPurse {
    fn cl_type() -> CLType {
        CLType::Any
    }
}

/// Validators and delegators mapped to their purses, validator/bidder key of origin, era of
/// withdrawal, tokens and expiration timer in eras.
pub type UnbondingPurses = BTreeMap<PublicKey, Vec<UnbondingPurse>>;