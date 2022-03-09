use alloc::{string::String, vec::Vec};
use borsh::{BorshSerialize, BorshDeserialize};
use core::{
    convert::{From, TryFrom},
    fmt::{Debug, Display, Formatter},
};
#[cfg(feature = "datasize")]
use datasize::DataSize;
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
#[cfg(feature = "json-schema")]
use schemars::{gen::SchemaGenerator, schema::Schema, JsonSchema};
use serde::{de::Error as SerdeError, Deserialize, Deserializer, Serialize, Serializer};

use super::FromStrError;
use crate::{
    checksummed_hex, crypto, CLType, CLTyped, PublicKey, BLAKE2B_DIGEST_LENGTH,
};

/// The length in bytes of a [`AccountHash`].
pub const ACCOUNT_HASH_LENGTH: usize = 32;
/// The prefix applied to the hex-encoded `AccountHash` to produce a formatted string
/// representation.
pub const ACCOUNT_HASH_FORMATTED_STRING_PREFIX: &str = "account-hash-";

/// A newtype wrapping an array which contains the raw bytes of
/// the AccountHash, a hash of Public Key and Algorithm
#[derive(Default, PartialOrd, Ord, PartialEq, Eq, Hash, Clone, Copy, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
pub struct AccountHash(pub [u8; ACCOUNT_HASH_LENGTH]);

impl AccountHash {
    /// Constructs a new `AccountHash` instance from the raw bytes of an Public Key Account Hash.
    pub const fn new(value: [u8; ACCOUNT_HASH_LENGTH]) -> AccountHash {
        AccountHash(value)
    }

    /// Returns the raw bytes of the account hash as an array.
    pub fn value(&self) -> [u8; ACCOUNT_HASH_LENGTH] {
        self.0
    }

    /// Returns the raw bytes of the account hash as a `slice`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Formats the `AccountHash` for users getting and putting.
    pub fn to_formatted_string(self) -> String {
        format!(
            "{}{}",
            ACCOUNT_HASH_FORMATTED_STRING_PREFIX,
            base16::encode_lower(&self.0),
        )
    }

    /// Parses a string formatted as per `Self::to_formatted_string()` into an `AccountHash`.
    pub fn from_formatted_str(input: &str) -> Result<Self, FromStrError> {
        let remainder = input
            .strip_prefix(ACCOUNT_HASH_FORMATTED_STRING_PREFIX)
            .ok_or(FromStrError::InvalidPrefix)?;
        let bytes =
            <[u8; ACCOUNT_HASH_LENGTH]>::try_from(checksummed_hex::decode(remainder)?.as_ref())?;
        Ok(AccountHash(bytes))
    }

    /// Parses a `PublicKey` and outputs the corresponding account hash.
    pub fn from_public_key(
        public_key: &PublicKey,
        blake2b_hash_fn: impl Fn(Vec<u8>) -> [u8; BLAKE2B_DIGEST_LENGTH],
    ) -> Self {
        const SYSTEM_LOWERCASE: &str = "system";
        const ED25519_LOWERCASE: &str = "ed25519";
        const SECP256K1_LOWERCASE: &str = "secp256k1";

        let algorithm_name = match public_key {
            PublicKey::System => SYSTEM_LOWERCASE,
            PublicKey::Ed25519(_) => ED25519_LOWERCASE,
            PublicKey::Secp256k1(_) => SECP256K1_LOWERCASE,
        };
        let public_key_bytes: Vec<u8> = public_key.into();

        // Prepare preimage based on the public key parameters.
        let preimage = {
            let mut data = Vec::with_capacity(algorithm_name.len() + public_key_bytes.len() + 1);
            data.extend(algorithm_name.as_bytes());
            data.push(0);
            data.extend(public_key_bytes);
            data
        };
        // Hash the preimage data using blake2b256 and return it.
        let digest = blake2b_hash_fn(preimage);
        Self::new(digest)
    }
}

#[cfg(feature = "json-schema")]
impl JsonSchema for AccountHash {
    fn schema_name() -> String {
        String::from("AccountHash")
    }

    fn json_schema(gen: &mut SchemaGenerator) -> Schema {
        let schema = gen.subschema_for::<String>();
        let mut schema_object = schema.into_object();
        schema_object.metadata().description = Some("Hex-encoded account hash.".to_string());
        schema_object.into()
    }
}

impl Serialize for AccountHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            Serialize::serialize(&self.to_formatted_string(), serializer)
        } else {
            Serialize::serialize(&self.0, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for AccountHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let formatted_string = <std::string::String as Deserialize>::deserialize(deserializer)?;
            AccountHash::from_formatted_str(&formatted_string).map_err(SerdeError::custom)
        } else {
            let bytes = <[u8; 32] as Deserialize>::deserialize(deserializer)?;
            Ok(AccountHash(bytes))
        }
    }
}

impl TryFrom<&[u8]> for AccountHash {
    type Error = TryFromSliceForAccountHashError;

    fn try_from(bytes: &[u8]) -> Result<Self, TryFromSliceForAccountHashError> {
        <[u8; ACCOUNT_HASH_LENGTH]>::try_from(bytes)
            .map(AccountHash::new)
            .map_err(|_| TryFromSliceForAccountHashError(()))
    }
}

impl TryFrom<&alloc::vec::Vec<u8>> for AccountHash {
    type Error = TryFromSliceForAccountHashError;

    fn try_from(bytes: &Vec<u8>) -> Result<Self, Self::Error> {
        <[u8; ACCOUNT_HASH_LENGTH]>::try_from(bytes as &[u8])
            .map(AccountHash::new)
            .map_err(|_| TryFromSliceForAccountHashError(()))
    }
}

impl From<&PublicKey> for AccountHash {
    fn from(public_key: &PublicKey) -> Self {
        AccountHash::from_public_key(public_key, crypto::blake2b)
    }
}

impl Display for AccountHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", base16::encode_lower(&self.0))
    }
}

impl Debug for AccountHash {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        write!(f, "AccountHash({})", base16::encode_lower(&self.0))
    }
}

impl CLTyped for AccountHash {
    fn cl_type() -> CLType {
        CLType::ByteArray(ACCOUNT_HASH_LENGTH as u32)
    }
}

impl AsRef<[u8]> for AccountHash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

/// Associated error type of `TryFrom<&[u8]>` for [`AccountHash`].
#[derive(Debug)]
pub struct TryFromSliceForAccountHashError(());

impl Distribution<AccountHash> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> AccountHash {
        AccountHash::new(rng.gen())
    }
}
