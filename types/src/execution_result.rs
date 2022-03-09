//! This file provides types to allow conversion from an EE `ExecutionResult` into a similar type
//! which can be serialized to a valid binary or JSON representation.
//!
//! It is stored as metadata related to a given deploy, and made available to clients via the
//! JSON-RPC API.

// TODO - remove once schemars stops causing warning.
#![allow(clippy::field_reassign_with_default)]

use core::convert::TryFrom;

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "datasize")]
use datasize::DataSize;
use num::{FromPrimitive, ToPrimitive};
use num_derive::{FromPrimitive, ToPrimitive};
#[cfg(feature = "json-schema")]
use once_cell::sync::Lazy;
use rand::{
    distributions::{Distribution, Standard},
    seq::SliceRandom,
    Rng,
};
#[cfg(feature = "json-schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(feature = "json-schema")]
use crate::KEY_HASH_LENGTH;
use crate::{
    account::AccountHash,
    bytesrepr::{self, FromBytes, ToBytes, U8_SERIALIZED_LENGTH},
    system::auction::{Bid, EraInfo, UnbondingPurse},
    CLValue, DeployInfo, NamedKey, Transfer, TransferAddr, U128, U256, U512,
};

#[derive(FromPrimitive, ToPrimitive, Debug)]
#[repr(u8)]
enum ExecutionResultTag {
    Failure = 0,
    Success = 1,
}

impl TryFrom<u8> for ExecutionResultTag {
    type Error = bytesrepr::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        FromPrimitive::from_u8(value).ok_or(bytesrepr::Error::Formatting)
    }
}

#[derive(FromPrimitive, ToPrimitive, Debug)]
#[repr(u8)]
enum OpTag {
    Read = 0,
    Write = 1,
    Add = 2,
    NoOp = 3,
}

impl TryFrom<u8> for OpTag {
    type Error = bytesrepr::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        FromPrimitive::from_u8(value).ok_or(bytesrepr::Error::Formatting)
    }
}

#[derive(FromPrimitive, ToPrimitive, Debug)]
#[repr(u8)]
enum TransformTag {
    Identity = 0,
    WriteCLValue = 1,
    WriteAccount = 2,
    WriteContractWasm = 3,
    WriteContract = 4,
    WriteContractPackage = 5,
    WriteDeployInfo = 6,
    WriteTransfer = 7,
    WriteEraInfo = 8,
    WriteBid = 9,
    WriteWithdraw = 10,
    AddInt32 = 11,
    AddUInt64 = 12,
    AddUInt128 = 13,
    AddUInt256 = 14,
    AddUInt512 = 15,
    AddKeys = 16,
    Failure = 17,
}

impl TryFrom<u8> for TransformTag {
    type Error = bytesrepr::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        FromPrimitive::from_u8(value).ok_or(bytesrepr::Error::Formatting)
    }
}

#[cfg(feature = "json-schema")]
static EXECUTION_RESULT: Lazy<ExecutionResult> = Lazy::new(|| {
    let operations = vec![
        Operation {
            key: "account-hash-2c4a11c062a8a337bfc97e27fd66291caeb2c65865dcb5d3ef3759c4c97efecb"
                .to_string(),
            kind: OpKind::Write,
        },
        Operation {
            key: "deploy-af684263911154d26fa05be9963171802801a0b6aff8f199b7391eacb8edc9e1"
                .to_string(),
            kind: OpKind::Read,
        },
    ];

    let transforms = vec![
        TransformEntry {
            key: "uref-2c4a11c062a8a337bfc97e27fd66291caeb2c65865dcb5d3ef3759c4c97efecb-007"
                .to_string(),
            transform: Transform::AddUInt64(8u64),
        },
        TransformEntry {
            key: "deploy-af684263911154d26fa05be9963171802801a0b6aff8f199b7391eacb8edc9e1"
                .to_string(),
            transform: Transform::Identity,
        },
    ];

    let effect = ExecutionEffect {
        operations,
        transforms,
    };

    let transfers = vec![
        TransferAddr::new([89; KEY_HASH_LENGTH]),
        TransferAddr::new([130; KEY_HASH_LENGTH]),
    ];

    ExecutionResult::Success {
        effect,
        transfers,
        cost: U512::from(123_456),
    }
});

/// The result of executing a single deploy.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub enum ExecutionResult {
    /// The result of a failed execution.
    Failure {
        /// The effect of executing the deploy.
        effect: ExecutionEffect,
        /// A record of Transfers performed while executing the deploy.
        transfers: Vec<TransferAddr>,
        /// The cost of executing the deploy.
        cost: U512,
        /// The error message associated with executing the deploy.
        error_message: String,
    },
    /// The result of a successful execution.
    Success {
        /// The effect of executing the deploy.
        effect: ExecutionEffect,
        /// A record of Transfers performed while executing the deploy.
        transfers: Vec<TransferAddr>,
        /// The cost of executing the deploy.
        cost: U512,
    },
}

impl ExecutionResult {
    // This method is not intended to be used by third party crates.
    #[doc(hidden)]
    #[cfg(feature = "json-schema")]
    pub fn example() -> &'static Self {
        &*EXECUTION_RESULT
    }

    fn tag_byte(&self) -> u8 {
        self.tag()
            .to_u8()
            .expect("ExecutionResultTag should be represented as u8")
    }

    fn tag(&self) -> ExecutionResultTag {
        match self {
            ExecutionResult::Failure {
                effect: _,
                transfers: _,
                cost: _,
                error_message: _,
            } => ExecutionResultTag::Failure,
            ExecutionResult::Success {
                effect: _,
                transfers: _,
                cost: _,
            } => ExecutionResultTag::Success,
        }
    }
}

impl Distribution<ExecutionResult> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> ExecutionResult {
        let op_count = rng.gen_range(0..6);
        let mut operations = Vec::new();
        for _ in 0..op_count {
            let op = [OpKind::Read, OpKind::Add, OpKind::NoOp, OpKind::Write]
                .choose(rng)
                .unwrap();
            operations.push(Operation {
                key: rng.gen::<u64>().to_string(),
                kind: *op,
            });
        }

        let transform_count = rng.gen_range(0..6);
        let mut transforms = Vec::new();
        for _ in 0..transform_count {
            transforms.push(TransformEntry {
                key: rng.gen::<u64>().to_string(),
                transform: rng.gen(),
            });
        }

        let execution_effect = ExecutionEffect::new(transforms);

        let transfer_count = rng.gen_range(0..6);
        let mut transfers = vec![];
        for _ in 0..transfer_count {
            transfers.push(TransferAddr::new(rng.gen()))
        }

        if rng.gen() {
            ExecutionResult::Failure {
                effect: execution_effect,
                transfers,
                cost: rng.gen::<u64>().into(),
                error_message: format!("Error message {}", rng.gen::<u64>()),
            }
        } else {
            ExecutionResult::Success {
                effect: execution_effect,
                transfers,
                cost: rng.gen::<u64>().into(),
            }
        }
    }
}

/// The journal of execution transforms from a single deploy.
#[derive(
    Clone, Eq, PartialEq, Serialize, Deserialize, Default, Debug, BorshSerialize, BorshDeserialize,
)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ExecutionEffect {
    /// The resulting operations.
    pub operations: Vec<Operation>,
    /// The journal of execution transforms.
    pub transforms: Vec<TransformEntry>,
}

impl ExecutionEffect {
    /// Constructor for [`ExecutionEffect`].
    pub fn new(transforms: Vec<TransformEntry>) -> Self {
        Self {
            transforms,
            operations: Default::default(),
        }
    }
}
/// An operation performed while executing a deploy.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Operation {
    /// The formatted string of the `Key`.
    pub key: String,
    /// The type of operation.
    pub kind: OpKind,
}

impl ToBytes for Operation {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        let mut buffer = bytesrepr::allocate_buffer(self)?;
        buffer.extend(self.key.to_bytes()?);
        buffer.extend(self.kind.to_bytes()?);
        Ok(buffer)
    }

    fn serialized_length(&self) -> usize {
        self.key.serialized_length() + self.kind.serialized_length()
    }
}

impl FromBytes for Operation {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (key, remainder) = String::from_bytes(bytes)?;
        let (kind, remainder) = OpKind::from_bytes(remainder)?;
        let operation = Operation { key, kind };
        Ok((operation, remainder))
    }
}

/// The type of operation performed while executing a deploy.
#[derive(
    Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug, BorshSerialize, BorshDeserialize,
)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub enum OpKind {
    /// A read operation.
    Read,
    /// A write operation.
    Write,
    /// An addition.
    Add,
    /// An operation which has no effect.
    NoOp,
}

impl OpKind {
    fn tag_byte(&self) -> u8 {
        self.tag()
            .to_u8()
            .expect("OpTag should be represented as u8")
    }

    fn tag(&self) -> OpTag {
        match self {
            OpKind::Read => OpTag::Read,
            OpKind::Write => OpTag::Write,
            OpKind::Add => OpTag::Add,
            OpKind::NoOp => OpTag::NoOp,
        }
    }
}

impl ToBytes for OpKind {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        self.tag_byte().to_bytes()
    }

    fn serialized_length(&self) -> usize {
        U8_SERIALIZED_LENGTH
    }
}

impl FromBytes for OpKind {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (tag, remainder) = u8::from_bytes(bytes)?;
        match TryFrom::try_from(tag)? {
            OpTag::Read => Ok((OpKind::Read, remainder)),
            OpTag::Write => Ok((OpKind::Write, remainder)),
            OpTag::Add => Ok((OpKind::Add, remainder)),
            OpTag::NoOp => Ok((OpKind::NoOp, remainder)),
        }
    }
}

/// A transformation performed while executing a deploy.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TransformEntry {
    /// The formatted string of the `Key`.
    pub key: String,
    /// The transformation.
    pub transform: Transform,
}

/// The actual transformation performed while executing a deploy.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug, BorshSerialize, BorshDeserialize)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[cfg_attr(feature = "json-schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub enum Transform {
    /// A transform having no effect.
    Identity,
    /// Writes the given CLValue to global state.
    WriteCLValue(CLValue),
    /// Writes the given Account to global state.
    WriteAccount(AccountHash),
    /// Writes a smart contract as Wasm to global state.
    WriteContractWasm,
    /// Writes a smart contract to global state.
    WriteContract,
    /// Writes a smart contract package to global state.
    WriteContractPackage,
    /// Writes the given DeployInfo to global state.
    WriteDeployInfo(DeployInfo),
    /// Writes the given EraInfo to global state.
    WriteEraInfo(EraInfo),
    /// Writes the given Transfer to global state.
    WriteTransfer(Transfer),
    /// Writes the given Bid to global state.
    WriteBid(Box<Bid>),
    /// Writes the given Withdraw to global state.
    WriteWithdraw(Vec<UnbondingPurse>),
    /// Adds the given `i32`.
    AddInt32(i32),
    /// Adds the given `u64`.
    AddUInt64(u64),
    /// Adds the given `U128`.
    AddUInt128(U128),
    /// Adds the given `U256`.
    AddUInt256(U256),
    /// Adds the given `U512`.
    AddUInt512(U512),
    /// Adds the given collection of named keys.
    AddKeys(Vec<NamedKey>),
    /// A failed transformation, containing an error message.
    Failure(String),
}

impl Transform {
    fn tag_byte(&self) -> u8 {
        self.tag()
            .to_u8()
            .expect("TransformTag should be representable as a u8")
    }

    fn tag(&self) -> TransformTag {
        match self {
            Transform::Identity => TransformTag::Identity,
            Transform::WriteCLValue(_) => TransformTag::WriteCLValue,
            Transform::WriteAccount(_) => TransformTag::WriteAccount,
            Transform::WriteContractWasm => TransformTag::WriteContractWasm,
            Transform::WriteContract => TransformTag::WriteContract,
            Transform::WriteContractPackage => TransformTag::WriteContractPackage,
            Transform::WriteDeployInfo(_) => TransformTag::WriteDeployInfo,
            Transform::WriteEraInfo(_) => TransformTag::WriteEraInfo,
            Transform::WriteTransfer(_) => TransformTag::WriteTransfer,
            Transform::WriteBid(_) => TransformTag::WriteBid,
            Transform::WriteWithdraw(_) => TransformTag::WriteWithdraw,
            Transform::AddInt32(_) => TransformTag::AddInt32,
            Transform::AddUInt64(_) => TransformTag::AddUInt64,
            Transform::AddUInt128(_) => TransformTag::AddUInt128,
            Transform::AddUInt256(_) => TransformTag::AddUInt256,
            Transform::AddUInt512(_) => TransformTag::AddUInt512,
            Transform::AddKeys(_) => TransformTag::AddKeys,
            Transform::Failure(_) => TransformTag::Failure,
        }
    }
}

impl Distribution<Transform> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Transform {
        // TODO - include WriteDeployInfo and WriteTransfer as options
        match rng.gen_range(0..13) {
            0 => Transform::Identity,
            1 => Transform::WriteCLValue(CLValue::from_t(true).unwrap()),
            2 => Transform::WriteAccount(AccountHash::new(rng.gen())),
            3 => Transform::WriteContractWasm,
            4 => Transform::WriteContract,
            5 => Transform::WriteContractPackage,
            6 => Transform::AddInt32(rng.gen()),
            7 => Transform::AddUInt64(rng.gen()),
            8 => Transform::AddUInt128(rng.gen::<u64>().into()),
            9 => Transform::AddUInt256(rng.gen::<u64>().into()),
            10 => Transform::AddUInt512(rng.gen::<u64>().into()),
            11 => {
                let mut named_keys = Vec::new();
                for _ in 0..rng.gen_range(1..6) {
                    let _ = named_keys.push(NamedKey {
                        name: rng.gen::<u64>().to_string(),
                        key: rng.gen::<u64>().to_string(),
                    });
                }
                Transform::AddKeys(named_keys)
            }
            12 => Transform::Failure(rng.gen::<u64>().to_string()),
            _ => unreachable!(),
        }
    }
}
