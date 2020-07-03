use parity_wasm::elements;
use thiserror::Error;

use crate::contract_shared::{wasm_prep, TypeMismatch};
use types::{
    account::{AddKeyFailure, RemoveKeyFailure, SetThresholdFailure, UpdateKeyFailure},
    bytesrepr, system_contract_errors, AccessRights, ApiError, CLType, CLValueError,
    ContractPackageHash, ContractVersionKey, Key, URef,
};

use crate::contract_core::resolvers::error::ResolverError;
use crate::contract_storage;

#[derive(Error, Debug, Clone)]
pub enum Error {
    #[error("Interpreter error: {}", _0)]
    Interpreter(String),
    #[error("Storage error: {}", _0)]
    Storage(contract_storage::error::Error),
    #[error("Serialization error: {}", _0)]
    BytesRepr(bytesrepr::Error),
    #[error("Named key {} not found", _0)]
    NamedKeyNotFound(String),
    #[error("Key {} not found", _0)]
    KeyNotFound(Key),
    #[error("Account {:?} not found", _0)]
    AccountNotFound(Key),
    #[error("{}", _0)]
    TypeMismatch(TypeMismatch),
    #[error("Invalid access rights: {}", required)]
    InvalidAccess { required: AccessRights },
    #[error("Forged reference: {}", _0)]
    ForgedReference(URef),
    #[error("URef not found: {}", _0)]
    URefNotFound(String),
    #[error("Function not found: {}", _0)]
    FunctionNotFound(String),
    #[error("{}", _0)]
    ParityWasm(elements::Error),
    #[error("Out of gas error")]
    GasLimit,
    #[error("Return")]
    Ret(Vec<URef>),
    #[error("{}", _0)]
    Rng(String),
    #[error("Resolver error: {}", _0)]
    Resolver(ResolverError),
    /// Reverts execution with a provided status
    #[error("{}", _0)]
    Revert(ApiError),
    #[error("{}", _0)]
    AddKeyFailure(AddKeyFailure),
    #[error("{}", _0)]
    RemoveKeyFailure(RemoveKeyFailure),
    #[error("{}", _0)]
    UpdateKeyFailure(UpdateKeyFailure),
    #[error("{}", _0)]
    SetThresholdFailure(SetThresholdFailure),
    #[error("{}", _0)]
    SystemContract(system_contract_errors::Error),
    #[error("Deployment authorization failure")]
    DeploymentAuthorizationFailure,
    #[error("Expected return value")]
    ExpectedReturnValue,
    #[error("Unexpected return value")]
    UnexpectedReturnValue,
    #[error("Invalid context")]
    InvalidContext,
    #[error("Incompatible protocol major version. Expected version {expected} but actual version is {actual}")]
    IncompatibleProtocolMajorVersion { expected: u32, actual: u32 },
    #[error("{0}")]
    CLValue(CLValueError),
    #[error("Host buffer is empty")]
    HostBufferEmpty,
    #[error("Unsupported WASM start")]
    UnsupportedWasmStart,
    #[error("No active contract versions for contract package")]
    NoActiveContractVersions(ContractPackageHash),
    #[error("Invalid contract version: {}", _0)]
    InvalidContractVersion(ContractVersionKey),
    #[error("No such method: {}", _0)]
    NoSuchMethod(String),
    #[error("Wasm preprocessing error: {}", _0)]
    WasmPreprocessing(wasm_prep::PreprocessingError),
    #[error("Unexpected Key length. Expected length {expected} but actual length is {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
}

impl From<wasm_prep::PreprocessingError> for Error {
    fn from(error: wasm_prep::PreprocessingError) -> Self {
        Error::WasmPreprocessing(error)
    }
}

impl Error {
    pub fn type_mismatch(expected: CLType, found: CLType) -> Error {
        Error::TypeMismatch(TypeMismatch {
            expected: format!("{:?}", expected),
            found: format!("{:?}", found),
        })
    }
}

impl wasmi::HostError for Error {}

impl From<wasmi::Error> for Error {
    fn from(error: wasmi::Error) -> Self {
        match error
            .as_host_error()
            .and_then(|host_error| host_error.downcast_ref::<Error>())
        {
            Some(error) => error.clone(),
            None => Error::Interpreter(error.into()),
        }
    }
}

impl From<contract_storage::error::Error> for Error {
    fn from(e: contract_storage::error::Error) -> Self {
        Error::Storage(e)
    }
}

impl From<bytesrepr::Error> for Error {
    fn from(e: bytesrepr::Error) -> Self {
        Error::BytesRepr(e)
    }
}

impl From<elements::Error> for Error {
    fn from(e: elements::Error) -> Self {
        Error::ParityWasm(e)
    }
}

impl From<ResolverError> for Error {
    fn from(err: ResolverError) -> Self {
        Error::Resolver(err)
    }
}

impl From<AddKeyFailure> for Error {
    fn from(err: AddKeyFailure) -> Self {
        Error::AddKeyFailure(err)
    }
}

impl From<RemoveKeyFailure> for Error {
    fn from(err: RemoveKeyFailure) -> Self {
        Error::RemoveKeyFailure(err)
    }
}

impl From<UpdateKeyFailure> for Error {
    fn from(err: UpdateKeyFailure) -> Self {
        Error::UpdateKeyFailure(err)
    }
}

impl From<SetThresholdFailure> for Error {
    fn from(err: SetThresholdFailure) -> Self {
        Error::SetThresholdFailure(err)
    }
}

impl From<system_contract_errors::Error> for Error {
    fn from(error: system_contract_errors::Error) -> Self {
        Error::SystemContract(error)
    }
}

impl From<CLValueError> for Error {
    fn from(e: CLValueError) -> Self {
        Error::CLValue(e)
    }
}