use alloc::vec::Vec;

#[cfg(feature = "datasize")]
use datasize::DataSize;
#[cfg(feature = "json-schema")]
use schemars::JsonSchema;
#[cfg(any(feature = "std", test))]
use serde::{Deserialize, Serialize};

use crate::{
    bytesrepr::{self, Bytes, FromBytes, ToBytes},
    RuntimeArgs, U512,
};

const NAMED_ARGUMENTS_TAG: u8 = 0;
const BYTES_TAG: u8 = 1;

/// Arguments of a `TransactionV1`.
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
#[cfg_attr(
    any(feature = "std", test),
    derive(Serialize, Deserialize),
    serde(deny_unknown_fields)
)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(
    feature = "json-schema",
    derive(JsonSchema),
    schemars(description = "Arguments of a `TransactionV1`.")
)]
pub enum TransactionArgs {
    /// Arguments passed by name.
    ///
    /// This is the most common way to pass arguments to a contract used in a `VmCasperV1` runtime.
    VmCasperV1(RuntimeArgs),
    /// Arguments passed as a single byte array.
    ///
    /// This is used in the `VmCasperV2` runtime.
    VmCasperV2 {
        /// The tokens that are available to use inside the execution environment for transfers.
        ///
        /// This is introduced in favor of V1's `named_args` and implicit "amount" argument that
        /// was used as token allowance.
        ///
        /// Passing 0 effectively removes the token usage from the execution environment, and the
        /// contract will not be able to transfer tokens. This is useful security measure
        /// to prevent accidental token transfers from possibly untrusted contract calls.
        ///
        /// Otherwise if the value is greater than 0, the contract will be able to transfer tokens
        /// up to the value specified. Additionally, the contract will be able to read the current
        /// balance of the caller using the `casper_env_value` host function.
        value: U512,
        /// The byte array containing the arguments.
        input: Bytes,
    },
}

impl ToBytes for TransactionArgs {
    fn write_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), bytesrepr::Error> {
        match self {
            TransactionArgs::VmCasperV1(named_args) => {
                bytes.push(NAMED_ARGUMENTS_TAG);
                named_args.write_bytes(bytes)?;
            }
            TransactionArgs::VmCasperV2 { value, input } => {
                bytes.push(BYTES_TAG);
                value.write_bytes(bytes)?;
                input.write_bytes(bytes)?;
            }
        }
        Ok(())
    }

    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        let mut bytes = Vec::new();
        self.write_bytes(&mut bytes)?;
        Ok(bytes)
    }

    fn serialized_length(&self) -> usize {
        1 + match self {
            TransactionArgs::VmCasperV1(named_args) => named_args.serialized_length(),
            TransactionArgs::VmCasperV2 { value, input } => {
                value.serialized_length() + input.serialized_length()
            }
        }
    }
}
impl FromBytes for TransactionArgs {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (tag, rem) = u8::from_bytes(bytes)?;
        match tag {
            NAMED_ARGUMENTS_TAG => {
                let (named_args, rem) = RuntimeArgs::from_bytes(rem)?;
                Ok((TransactionArgs::VmCasperV1(named_args), rem))
            }
            BYTES_TAG => {
                let (value, rem) = U512::from_bytes(rem)?;
                let (input, rem) = Bytes::from_bytes(rem)?;
                Ok((TransactionArgs::VmCasperV2 { value, input }, rem))
            }
            _ => Err(bytesrepr::Error::Formatting),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrip() {
        let named_args = RuntimeArgs::new();
        let input = TransactionArgs::VmCasperV1(named_args);
        bytesrepr::test_serialization_roundtrip(&input);

        let input = Bytes::from(vec![1, 2, 3]);
        let input = TransactionArgs::VmCasperV2 {
            value: U512::from(u64::MAX) + U512::one(),
            input,
        };
        bytesrepr::test_serialization_roundtrip(&input);
    }
}
