use alloc::vec::Vec;

#[cfg(feature = "datasize")]
use datasize::DataSize;
#[cfg(feature = "json-schema")]
use schemars::JsonSchema;
#[cfg(any(feature = "std", test))]
use serde::{Deserialize, Serialize};

use crate::{
    bytesrepr::{self, Bytes, FromBytes, ToBytes},
    RuntimeArgs,
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
    NamedArguments(RuntimeArgs),
    /// Arguments passed as a single byte array.
    ///
    /// This is used in the `VmCapserV2` runtime.
    Bytes(Bytes),
}

impl ToBytes for TransactionArgs {
    fn write_bytes(&self, bytes: &mut Vec<u8>) -> Result<(), bytesrepr::Error> {
        match self {
            TransactionArgs::NamedArguments(named_args) => {
                bytes.push(NAMED_ARGUMENTS_TAG);
                named_args.write_bytes(bytes)?;
            }
            TransactionArgs::Bytes(input_bytes) => {
                bytes.push(BYTES_TAG);
                input_bytes.write_bytes(bytes)?;
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
            TransactionArgs::NamedArguments(named_args) => named_args.serialized_length(),
            TransactionArgs::Bytes(bytes) => bytes.serialized_length(),
        }
    }
}
impl FromBytes for TransactionArgs {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (tag, rem) = u8::from_bytes(bytes)?;
        match tag {
            NAMED_ARGUMENTS_TAG => {
                let (named_args, rem) = RuntimeArgs::from_bytes(rem)?;
                Ok((TransactionArgs::NamedArguments(named_args), rem))
            }
            BYTES_TAG => {
                let (bytes, rem) = Bytes::from_bytes(rem)?;
                Ok((TransactionArgs::Bytes(bytes), rem))
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
        let input = TransactionArgs::NamedArguments(named_args);
        bytesrepr::test_serialization_roundtrip(&input);

        let bytes = Bytes::from(vec![1, 2, 3]);
        let input = TransactionArgs::Bytes(bytes);
        bytesrepr::test_serialization_roundtrip(&input);
    }
}
