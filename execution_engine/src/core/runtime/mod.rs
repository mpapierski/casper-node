//! This module contains executor state of the WASM code.
mod auction_internal;
mod externals;
mod handle_payment_internal;
mod mint_internal;
mod scoped_instrumenter;
mod standard_payment_internal;
mod wasmi_args_parser;

use std::{
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    convert::TryFrom,
    iter::IntoIterator,
};

use itertools::Itertools;
use tracing::error;
use wasmi::{ImportsBuilder, MemoryRef, ModuleInstance, ModuleRef};

use casper_types::{
    account::{self, Account, AccountHash, ActionType, Weight},
    api_error,
    bytesrepr::{self, FromBytes, ToBytes},
    contracts::{
        self, Contract, ContractPackage, ContractPackageStatus, ContractVersion, ContractVersions,
        DisabledVersions, EntryPoint, EntryPointAccess, EntryPoints, Group, Groups, NamedKeys,
    },
    system::{
        self,
        auction::{self, Auction, EraInfo},
        handle_payment::{self, HandlePayment},
        mint::{self, Mint},
        standard_payment::{self, StandardPayment},
        CallStackElement, SystemContractType, AUCTION, HANDLE_PAYMENT, MINT, STANDARD_PAYMENT,
    },
    AccessRights, ApiError, CLType, CLTyped, CLValue, ContractHash, ContractPackageHash,
    ContractVersionKey, ContractWasm, DeployHash, EntryPointType, EraId, Gas, Key, NamedArg,
    Parameter, Phase, ProtocolVersion, PublicKey, RuntimeArgs, StoredValue, Transfer,
    TransferResult, TransferredTo, URef, DICTIONARY_ITEM_KEY_MAX_LENGTH, U128, U256, U512,
};

use crate::{
    core::{
        engine_state::{system_contract_cache::SystemContractCache, EngineConfig},
        execution::{self, Error},
        resolvers::{create_module_resolver, memory_resolver::MemoryResolver},
        runtime::scoped_instrumenter::ScopedInstrumenter,
        runtime_context::{self, RuntimeContext},
        Address,
    },
    shared::{
        host_function_costs::{Cost, HostFunction, DEFAULT_HOST_FUNCTION_NEW_DICTIONARY},
        wasm_config::WasmConfig,
        wasm_engine::{ExecutionMode, Instance, InstanceRef, Module, RuntimeValue, WasmEngine},
    },
    storage::global_state::StateReader,
};

use super::resolvers::v1_function_index::FunctionIndex;

/// Represents the runtime properties of a WASM execution.
pub struct Runtime<'a, R> {
    system_contract_cache: SystemContractCache,
    config: EngineConfig,
    module: Module,
    instance: Instance,
    host_buffer: Option<CLValue>,
    context: RuntimeContext<'a, R>,
    call_stack: Vec<CallStackElement>,
    wasm_engine: &'a WasmEngine,
}

/// Creates an WASM module instance and a memory instance.
///
/// This ensures that a memory instance is properly resolved into a pre-allocated memory area, and a
/// host function resolver is attached to the module.
///
/// The WASM module is also validated to not have a "start" section as we currently don't support
/// running it.
///
/// Both [`ModuleRef`] and a [`MemoryRef`] are ready to be executed.
pub fn instance_and_memory(
    wasm_module: Module,
    protocol_version: ProtocolVersion,
    wasm_engine: &WasmEngine,
) -> Result<Instance, Error> {
    match wasm_engine.execution_mode() {
        ExecutionMode::Interpreted => {
            let wasmi_module = wasm_module
                .try_into_interpreted()
                .expect("expected interpreted wasm module");
            let module = wasmi::Module::from_parity_wasm_module(wasmi_module)?;
            let resolver = create_module_resolver(protocol_version, wasm_engine.wasm_config())?;
            let mut imports = ImportsBuilder::new();
            imports.push_resolver("env", &resolver);
            let not_started_module = ModuleInstance::new(&module, &imports)?;
            if not_started_module.has_start() {
                return Err(Error::UnsupportedWasmStart);
            }
            let instance = not_started_module.not_started_instance().clone();
            let memory = resolver.memory_ref()?;
            Ok(Instance::Interpreted(instance, memory))
        }
        ExecutionMode::Compiled => {
            todo!("compiled mode")
        }
    }
}

/// Turns `key` into a `([u8; 32], AccessRights)` tuple.
/// Returns None if `key` is not `Key::URef` as it wouldn't have `AccessRights`
/// associated with it. Helper function for creating `named_keys` associating
/// addresses and corresponding `AccessRights`.
pub fn key_to_tuple(key: Key) -> Option<([u8; 32], AccessRights)> {
    match key {
        Key::URef(uref) => Some((uref.addr(), uref.access_rights())),
        Key::Account(_) => None,
        Key::Hash(_) => None,
        Key::Transfer(_) => None,
        Key::DeployInfo(_) => None,
        Key::EraInfo(_) => None,
        Key::Balance(_) => None,
        Key::Bid(_) => None,
        Key::Withdraw(_) => None,
        Key::Dictionary(_) => None,
        Key::SystemContractRegistry => None,
    }
}

/// Groups a collection of urefs by their addresses and accumulates access
/// rights per key
pub fn extract_access_rights_from_urefs<I: IntoIterator<Item = URef>>(
    input: I,
) -> HashMap<Address, HashSet<AccessRights>> {
    input
        .into_iter()
        .map(|uref: URef| (uref.addr(), uref.access_rights()))
        .group_by(|(key, _)| *key)
        .into_iter()
        .map(|(key, group)| {
            (
                key,
                group.map(|(_, x)| x).collect::<HashSet<AccessRights>>(),
            )
        })
        .collect()
}

/// Groups a collection of keys by their address and accumulates access rights
/// per key.
pub fn extract_access_rights_from_keys<I: IntoIterator<Item = Key>>(
    input: I,
) -> HashMap<Address, HashSet<AccessRights>> {
    input
        .into_iter()
        .map(key_to_tuple)
        .flatten()
        .group_by(|(key, _)| *key)
        .into_iter()
        .map(|(key, group)| {
            (
                key,
                group.map(|(_, x)| x).collect::<HashSet<AccessRights>>(),
            )
        })
        .collect()
}

#[allow(clippy::cognitive_complexity)]
fn extract_urefs(cl_value: &CLValue) -> Result<Vec<URef>, Error> {
    match cl_value.cl_type() {
        CLType::Bool
        | CLType::I32
        | CLType::I64
        | CLType::U8
        | CLType::U32
        | CLType::U64
        | CLType::U128
        | CLType::U256
        | CLType::U512
        | CLType::Unit
        | CLType::String
        | CLType::PublicKey
        | CLType::Any => Ok(vec![]),
        CLType::Option(ty) => match **ty {
            CLType::URef => {
                let opt: Option<URef> = cl_value.to_owned().into_t()?;
                Ok(opt.into_iter().collect())
            }
            CLType::Key => {
                let opt: Option<Key> = cl_value.to_owned().into_t()?;
                Ok(opt.into_iter().flat_map(Key::into_uref).collect())
            }
            _ => Ok(vec![]),
        },
        CLType::List(ty) => match **ty {
            CLType::URef => Ok(cl_value.to_owned().into_t()?),
            CLType::Key => {
                let keys: Vec<Key> = cl_value.to_owned().into_t()?;
                Ok(keys.into_iter().filter_map(Key::into_uref).collect())
            }
            _ => Ok(vec![]),
        },
        CLType::ByteArray(_) => Ok(vec![]),
        CLType::Result { ok, err } => match (&**ok, &**err) {
            (CLType::URef, CLType::Bool) => {
                let res: Result<URef, bool> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::I32) => {
                let res: Result<URef, i32> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::I64) => {
                let res: Result<URef, i64> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U8) => {
                let res: Result<URef, u8> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U32) => {
                let res: Result<URef, u32> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U64) => {
                let res: Result<URef, u64> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U128) => {
                let res: Result<URef, U128> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U256) => {
                let res: Result<URef, U256> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::U512) => {
                let res: Result<URef, U512> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::Unit) => {
                let res: Result<URef, ()> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::String) => {
                let res: Result<URef, String> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::URef, CLType::Key) => {
                let res: Result<URef, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::URef, CLType::URef) => {
                let res: Result<URef, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(uref) => Ok(vec![uref]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::Key, CLType::Bool) => {
                let res: Result<Key, bool> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::I32) => {
                let res: Result<Key, i32> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::I64) => {
                let res: Result<Key, i64> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U8) => {
                let res: Result<Key, u8> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U32) => {
                let res: Result<Key, u32> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U64) => {
                let res: Result<Key, u64> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U128) => {
                let res: Result<Key, U128> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U256) => {
                let res: Result<Key, U256> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::U512) => {
                let res: Result<Key, U512> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::Unit) => {
                let res: Result<Key, ()> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::String) => {
                let res: Result<Key, String> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(_) => Ok(vec![]),
                }
            }
            (CLType::Key, CLType::URef) => {
                let res: Result<Key, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::Key, CLType::Key) => {
                let res: Result<Key, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(key) => Ok(key.into_uref().into_iter().collect()),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::Bool, CLType::URef) => {
                let res: Result<bool, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::I32, CLType::URef) => {
                let res: Result<i32, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::I64, CLType::URef) => {
                let res: Result<i64, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U8, CLType::URef) => {
                let res: Result<u8, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U32, CLType::URef) => {
                let res: Result<u32, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U64, CLType::URef) => {
                let res: Result<u64, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U128, CLType::URef) => {
                let res: Result<U128, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U256, CLType::URef) => {
                let res: Result<U256, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::U512, CLType::URef) => {
                let res: Result<U512, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::Unit, CLType::URef) => {
                let res: Result<(), URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::String, CLType::URef) => {
                let res: Result<String, URef> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(uref) => Ok(vec![uref]),
                }
            }
            (CLType::Bool, CLType::Key) => {
                let res: Result<bool, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::I32, CLType::Key) => {
                let res: Result<i32, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::I64, CLType::Key) => {
                let res: Result<i64, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U8, CLType::Key) => {
                let res: Result<u8, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U32, CLType::Key) => {
                let res: Result<u32, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U64, CLType::Key) => {
                let res: Result<u64, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U128, CLType::Key) => {
                let res: Result<U128, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U256, CLType::Key) => {
                let res: Result<U256, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::U512, CLType::Key) => {
                let res: Result<U512, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::Unit, CLType::Key) => {
                let res: Result<(), Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (CLType::String, CLType::Key) => {
                let res: Result<String, Key> = cl_value.to_owned().into_t()?;
                match res {
                    Ok(_) => Ok(vec![]),
                    Err(key) => Ok(key.into_uref().into_iter().collect()),
                }
            }
            (_, _) => Ok(vec![]),
        },
        CLType::Map { key, value } => match (&**key, &**value) {
            (CLType::URef, CLType::Bool) => {
                let map: BTreeMap<URef, bool> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::I32) => {
                let map: BTreeMap<URef, i32> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::I64) => {
                let map: BTreeMap<URef, i64> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U8) => {
                let map: BTreeMap<URef, u8> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U32) => {
                let map: BTreeMap<URef, u32> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U64) => {
                let map: BTreeMap<URef, u64> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U128) => {
                let map: BTreeMap<URef, U128> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U256) => {
                let map: BTreeMap<URef, U256> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::U512) => {
                let map: BTreeMap<URef, U512> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::Unit) => {
                let map: BTreeMap<URef, ()> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::String) => {
                let map: BTreeMap<URef, String> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().collect())
            }
            (CLType::URef, CLType::Key) => {
                let map: BTreeMap<URef, Key> = cl_value.to_owned().into_t()?;
                Ok(map
                    .keys()
                    .cloned()
                    .chain(map.values().cloned().filter_map(Key::into_uref))
                    .collect())
            }
            (CLType::URef, CLType::URef) => {
                let map: BTreeMap<URef, URef> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().chain(map.values().cloned()).collect())
            }
            (CLType::Key, CLType::Bool) => {
                let map: BTreeMap<Key, bool> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::I32) => {
                let map: BTreeMap<Key, i32> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::I64) => {
                let map: BTreeMap<Key, i64> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U8) => {
                let map: BTreeMap<Key, u8> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U32) => {
                let map: BTreeMap<Key, u32> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U64) => {
                let map: BTreeMap<Key, u64> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U128) => {
                let map: BTreeMap<Key, U128> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U256) => {
                let map: BTreeMap<Key, U256> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::U512) => {
                let map: BTreeMap<Key, U512> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::Unit) => {
                let map: BTreeMap<Key, ()> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::String) => {
                let map: BTreeMap<Key, String> = cl_value.to_owned().into_t()?;
                Ok(map.keys().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Key, CLType::URef) => {
                let map: BTreeMap<Key, URef> = cl_value.to_owned().into_t()?;
                Ok(map
                    .keys()
                    .cloned()
                    .filter_map(Key::into_uref)
                    .chain(map.values().cloned())
                    .collect())
            }
            (CLType::Key, CLType::Key) => {
                let map: BTreeMap<Key, Key> = cl_value.to_owned().into_t()?;
                Ok(map
                    .keys()
                    .cloned()
                    .filter_map(Key::into_uref)
                    .chain(map.values().cloned().filter_map(Key::into_uref))
                    .collect())
            }
            (CLType::Bool, CLType::URef) => {
                let map: BTreeMap<bool, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::I32, CLType::URef) => {
                let map: BTreeMap<i32, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::I64, CLType::URef) => {
                let map: BTreeMap<i64, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U8, CLType::URef) => {
                let map: BTreeMap<u8, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U32, CLType::URef) => {
                let map: BTreeMap<u32, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U64, CLType::URef) => {
                let map: BTreeMap<u64, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U128, CLType::URef) => {
                let map: BTreeMap<U128, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U256, CLType::URef) => {
                let map: BTreeMap<U256, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::U512, CLType::URef) => {
                let map: BTreeMap<U512, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::Unit, CLType::URef) => {
                let map: BTreeMap<(), URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::String, CLType::URef) => {
                let map: BTreeMap<String, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::PublicKey, CLType::URef) => {
                let map: BTreeMap<PublicKey, URef> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().collect())
            }
            (CLType::Bool, CLType::Key) => {
                let map: BTreeMap<bool, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::I32, CLType::Key) => {
                let map: BTreeMap<i32, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::I64, CLType::Key) => {
                let map: BTreeMap<i64, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U8, CLType::Key) => {
                let map: BTreeMap<u8, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U32, CLType::Key) => {
                let map: BTreeMap<u32, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U64, CLType::Key) => {
                let map: BTreeMap<u64, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U128, CLType::Key) => {
                let map: BTreeMap<U128, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U256, CLType::Key) => {
                let map: BTreeMap<U256, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::U512, CLType::Key) => {
                let map: BTreeMap<U512, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::Unit, CLType::Key) => {
                let map: BTreeMap<(), Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::String, CLType::Key) => {
                let map: NamedKeys = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (CLType::PublicKey, CLType::Key) => {
                let map: BTreeMap<PublicKey, Key> = cl_value.to_owned().into_t()?;
                Ok(map.values().cloned().filter_map(Key::into_uref).collect())
            }
            (_, _) => Ok(vec![]),
        },
        CLType::Tuple1([ty]) => match **ty {
            CLType::URef => {
                let val: (URef,) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            CLType::Key => {
                let val: (Key,) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            _ => Ok(vec![]),
        },
        CLType::Tuple2([ty1, ty2]) => match (&**ty1, &**ty2) {
            (CLType::URef, CLType::Bool) => {
                let val: (URef, bool) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::I32) => {
                let val: (URef, i32) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::I64) => {
                let val: (URef, i64) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U8) => {
                let val: (URef, u8) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U32) => {
                let val: (URef, u32) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U64) => {
                let val: (URef, u64) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U128) => {
                let val: (URef, U128) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U256) => {
                let val: (URef, U256) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::U512) => {
                let val: (URef, U512) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::Unit) => {
                let val: (URef, ()) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::String) => {
                let val: (URef, String) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0])
            }
            (CLType::URef, CLType::Key) => {
                let val: (URef, Key) = cl_value.to_owned().into_t()?;
                let mut res = vec![val.0];
                res.extend(val.1.into_uref().into_iter());
                Ok(res)
            }
            (CLType::URef, CLType::URef) => {
                let val: (URef, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.0, val.1])
            }
            (CLType::Key, CLType::Bool) => {
                let val: (Key, bool) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::I32) => {
                let val: (Key, i32) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::I64) => {
                let val: (Key, i64) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U8) => {
                let val: (Key, u8) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U32) => {
                let val: (Key, u32) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U64) => {
                let val: (Key, u64) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U128) => {
                let val: (Key, U128) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U256) => {
                let val: (Key, U256) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::U512) => {
                let val: (Key, U512) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::Unit) => {
                let val: (Key, ()) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::String) => {
                let val: (Key, String) = cl_value.to_owned().into_t()?;
                Ok(val.0.into_uref().into_iter().collect())
            }
            (CLType::Key, CLType::URef) => {
                let val: (Key, URef) = cl_value.to_owned().into_t()?;
                let mut res: Vec<URef> = val.0.into_uref().into_iter().collect();
                res.push(val.1);
                Ok(res)
            }
            (CLType::Key, CLType::Key) => {
                let val: (Key, Key) = cl_value.to_owned().into_t()?;
                Ok(val
                    .0
                    .into_uref()
                    .into_iter()
                    .chain(val.1.into_uref().into_iter())
                    .collect())
            }
            (CLType::Bool, CLType::URef) => {
                let val: (bool, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::I32, CLType::URef) => {
                let val: (i32, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::I64, CLType::URef) => {
                let val: (i64, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U8, CLType::URef) => {
                let val: (u8, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U32, CLType::URef) => {
                let val: (u32, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U64, CLType::URef) => {
                let val: (u64, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U128, CLType::URef) => {
                let val: (U128, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U256, CLType::URef) => {
                let val: (U256, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::U512, CLType::URef) => {
                let val: (U512, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::Unit, CLType::URef) => {
                let val: ((), URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::String, CLType::URef) => {
                let val: (String, URef) = cl_value.to_owned().into_t()?;
                Ok(vec![val.1])
            }
            (CLType::Bool, CLType::Key) => {
                let val: (bool, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::I32, CLType::Key) => {
                let val: (i32, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::I64, CLType::Key) => {
                let val: (i64, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U8, CLType::Key) => {
                let val: (u8, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U32, CLType::Key) => {
                let val: (u32, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U64, CLType::Key) => {
                let val: (u64, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U128, CLType::Key) => {
                let val: (U128, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U256, CLType::Key) => {
                let val: (U256, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::U512, CLType::Key) => {
                let val: (U512, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::Unit, CLType::Key) => {
                let val: ((), Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (CLType::String, CLType::Key) => {
                let val: (String, Key) = cl_value.to_owned().into_t()?;
                Ok(val.1.into_uref().into_iter().collect())
            }
            (_, _) => Ok(vec![]),
        },
        // TODO: nested matches for Tuple3?
        CLType::Tuple3(_) => Ok(vec![]),
        CLType::Key => {
            let key: Key = cl_value.to_owned().into_t()?; // TODO: optimize?
            Ok(key.into_uref().into_iter().collect())
        }
        CLType::URef => {
            let uref: URef = cl_value.to_owned().into_t()?; // TODO: optimize?
            Ok(vec![uref])
        }
    }
}

impl<'a, R> Runtime<'a, R>
where
    R: StateReader<Key, StoredValue>,
    R::Error: Into<Error>,
{
    /// Creates a new runtime instance.
    pub fn new(
        config: EngineConfig,
        system_contract_cache: SystemContractCache,
        module: Module,
        instance: Instance,
        context: RuntimeContext<'a, R>,
        call_stack: Vec<CallStackElement>,
        wasm_engine: &'a WasmEngine,
    ) -> Self {
        Runtime {
            config,
            system_contract_cache,
            module,
            instance,
            host_buffer: None,
            context,
            call_stack,
            wasm_engine,
        }
    }

    /// Returns a Wasm module instance.
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Returns the context.
    pub fn context(&self) -> &RuntimeContext<'a, R> {
        &self.context
    }

    fn gas(&mut self, amount: Gas) -> Result<(), Error> {
        self.context.charge_gas(amount)
    }

    /// Returns current gas counter.
    fn gas_counter(&self) -> Gas {
        self.context.gas_counter()
    }

    /// Sets new gas counter value.
    fn set_gas_counter(&mut self, new_gas_counter: Gas) {
        self.context.set_gas_counter(new_gas_counter);
    }

    /// Charge for a system contract call.
    pub(crate) fn charge_system_contract_call<T>(&mut self, amount: T) -> Result<(), Error>
    where
        T: Into<Gas>,
    {
        self.context.charge_system_contract_call(amount)
    }

    /// Returns current call stack.
    pub fn call_stack(&self) -> &Vec<CallStackElement> {
        &self.call_stack
    }

    /// Returns bytes from the WASM memory instance.
    fn bytes_from_mem(&self, ptr: u32, size: usize) -> Result<Vec<u8>, Error> {
        self.memory().get(ptr, size).map_err(Into::into)
    }

    /// Returns a deserialized type from the WASM memory instance.
    fn t_from_mem<T: FromBytes>(&self, ptr: u32, size: u32) -> Result<T, Error> {
        let bytes = self.bytes_from_mem(ptr, size as usize)?;
        bytesrepr::deserialize(bytes).map_err(Into::into)
    }

    /// Reads key (defined as `key_ptr` and `key_size` tuple) from Wasm memory.
    fn key_from_mem(&mut self, key_ptr: u32, key_size: u32) -> Result<Key, Error> {
        let bytes = self.bytes_from_mem(key_ptr, key_size as usize)?;
        bytesrepr::deserialize(bytes).map_err(Into::into)
    }

    /// Reads `CLValue` (defined as `cl_value_ptr` and `cl_value_size` tuple) from Wasm memory.
    fn cl_value_from_mem(
        &mut self,
        cl_value_ptr: u32,
        cl_value_size: u32,
    ) -> Result<CLValue, Error> {
        let bytes = self.bytes_from_mem(cl_value_ptr, cl_value_size as usize)?;
        bytesrepr::deserialize(bytes).map_err(Into::into)
    }

    /// Returns a deserialized string from the WASM memory instance.
    fn string_from_mem(&self, ptr: u32, size: u32) -> Result<String, Error> {
        let bytes = self.bytes_from_mem(ptr, size as usize)?;
        bytesrepr::deserialize(bytes).map_err(|e| Error::BytesRepr(e).into())
    }

    fn get_module_from_entry_points(
        &mut self,
        entry_points: &EntryPoints,
    ) -> Result<Vec<u8>, Error> {
        let mut parity_wasm = self.module.clone().into_interpreted()?;

        let export_section = parity_wasm
            .export_section()
            .ok_or_else(|| Error::FunctionNotFound(String::from("Missing Export Section")))?;

        let entry_point_names: Vec<&str> = entry_points.keys().map(|s| s.as_str()).collect();

        let maybe_missing_name: Option<String> = entry_point_names
            .iter()
            .find(|name| {
                !export_section
                    .entries()
                    .iter()
                    .any(|export_entry| export_entry.field() == **name)
            })
            .map(|s| String::from(*s));

        if let Some(missing_name) = maybe_missing_name {
            Err(Error::FunctionNotFound(missing_name))
        } else {
            let mut module = self.module.clone().into_interpreted()?;
            pwasm_utils::optimize(&mut module, entry_point_names)?;
            parity_wasm::serialize(module).map_err(Error::ParityWasm)
        }
    }

    fn is_valid_uref(&mut self, uref_ptr: u32, uref_size: u32) -> Result<bool, Error> {
        let bytes = self.bytes_from_mem(uref_ptr, uref_size as usize)?;
        let uref: URef = bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?;
        Ok(self.context.validate_uref(&uref).is_ok())
    }

    /// Load the uref known by the given name into the Wasm memory
    fn load_key(
        &mut self,
        name_ptr: u32,
        name_size: u32,
        output_ptr: u32,
        output_size: usize,
        bytes_written_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let name = self.string_from_mem(name_ptr, name_size)?;

        // Get a key and serialize it
        let key = match self.context.named_keys_get(&name) {
            Some(key) => key,
            None => return Ok(Err(ApiError::MissingKey)),
        };

        let key_bytes = match key.to_bytes() {
            Ok(bytes) => bytes,
            Err(error) => return Ok(Err(error.into())),
        };

        // `output_size` has to be greater or equal to the actual length of serialized Key bytes
        if output_size < key_bytes.len() {
            return Ok(Err(ApiError::BufferTooSmall));
        }

        // Set serialized Key bytes into the output buffer
        if let Err(error) = self.memory().set(output_ptr, &key_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        // For all practical purposes following cast is assumed to be safe
        let bytes_size = key_bytes.len() as u32;
        let size_bytes = bytes_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(bytes_written_ptr, &size_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        Ok(Ok(()))
    }

    fn has_key(&mut self, name_ptr: u32, name_size: u32) -> Result<i32, Error> {
        let name = self.string_from_mem(name_ptr, name_size)?;
        if self.context.named_keys_contains_key(&name) {
            Ok(0)
        } else {
            Ok(1)
        }
    }

    fn put_key(
        &mut self,
        name_ptr: u32,
        name_size: u32,
        key_ptr: u32,
        key_size: u32,
    ) -> Result<(), Error> {
        let name = self.string_from_mem(name_ptr, name_size)?;
        let key = self.key_from_mem(key_ptr, key_size)?;
        self.context.put_key(name, key).map_err(Into::into)
    }

    fn remove_key(&mut self, name_ptr: u32, name_size: u32) -> Result<(), Error> {
        let name = self.string_from_mem(name_ptr, name_size)?;
        self.context.remove_key(&name)?;
        Ok(())
    }

    /// Writes runtime context's account main purse to dest_ptr in the Wasm memory.
    fn get_main_purse(&mut self, dest_ptr: u32) -> Result<(), Error> {
        let purse = self.context.get_main_purse()?;
        let purse_bytes = purse.into_bytes().map_err(Error::BytesRepr)?;
        self.memory()
            .set(dest_ptr, &purse_bytes)
            .map_err(|e| Error::Interpreter(e.into()).into())
    }

    /// Writes caller (deploy) account public key to dest_ptr in the Wasm
    /// memory.
    fn get_caller(&mut self, output_size: u32) -> Result<Result<(), ApiError>, Error> {
        if !self.can_write_to_host_buffer() {
            // Exit early if the host buffer is already occupied
            return Ok(Err(ApiError::HostBufferFull));
        }
        let value = CLValue::from_t(self.context.get_caller()).map_err(Error::CLValue)?;
        let value_size = value.inner_bytes().len();

        // Save serialized public key into host buffer
        if let Err(error) = self.write_host_buffer(value) {
            return Ok(Err(error));
        }

        // Write output
        let output_size_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size, &output_size_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }
        Ok(Ok(()))
    }

    /// Gets the immediate caller of the current execution
    fn get_immediate_caller(&self) -> Option<&CallStackElement> {
        let call_stack = self.call_stack();
        let mut call_stack_iter = call_stack.iter().rev();
        call_stack_iter.next()?;
        call_stack_iter.next()
    }

    /// Checks if immediate caller is of session type of the same account as the provided account
    /// hash.
    fn is_allowed_session_caller(&self, provided_account_hash: &AccountHash) -> bool {
        if let Some(CallStackElement::Session { account_hash }) = self.get_immediate_caller() {
            return account_hash == provided_account_hash;
        }
        false
    }

    /// Writes runtime context's phase to dest_ptr in the Wasm memory.
    fn get_phase(&mut self, dest_ptr: u32) -> Result<(), Error> {
        let phase = self.context.phase();
        let bytes = phase.into_bytes().map_err(Error::BytesRepr)?;
        self.memory()
            .set(dest_ptr, &bytes)
            .map_err(|e| Error::Interpreter(e.into()).into())
    }

    /// Writes current blocktime to dest_ptr in Wasm memory.
    fn get_blocktime(&self, dest_ptr: u32) -> Result<(), Error> {
        let blocktime = self
            .context
            .get_blocktime()
            .into_bytes()
            .map_err(Error::BytesRepr)?;
        self.memory()
            .set(dest_ptr, &blocktime)
            .map_err(|e| Error::Interpreter(e.into()).into())
    }

    /// Load the uref known by the given name into the Wasm memory
    fn load_call_stack(
        &mut self,
        // (Output) Pointer to number of elements in the call stack.
        call_stack_len_ptr: u32,
        // (Output) Pointer to size in bytes of the serialized call stack.
        result_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        if !self.can_write_to_host_buffer() {
            // Exit early if the host buffer is already occupied
            return Ok(Err(ApiError::HostBufferFull));
        }
        let call_stack = self.call_stack();
        let call_stack_len = call_stack.len() as u32;
        let call_stack_len_bytes = call_stack_len.to_le_bytes();

        if let Err(error) = self.memory().set(call_stack_len_ptr, &call_stack_len_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        if call_stack_len == 0 {
            return Ok(Ok(()));
        }

        let call_stack_cl_value = CLValue::from_t(call_stack.clone()).map_err(Error::CLValue)?;

        let call_stack_cl_value_bytes_len = call_stack_cl_value.inner_bytes().len() as u32;
        if let Err(error) = self.write_host_buffer(call_stack_cl_value) {
            return Ok(Err(error));
        }

        let call_stack_cl_value_bytes_len_bytes = call_stack_cl_value_bytes_len.to_le_bytes();

        if let Err(error) = self
            .memory()
            .set(result_size_ptr, &call_stack_cl_value_bytes_len_bytes)
        {
            return Err(Error::Interpreter(error.into()).into());
        }

        Ok(Ok(()))
    }

    /// Return some bytes from the memory and terminate the current `sub_call`. Note that the return
    /// type is `Trap`, indicating that this function will always kill the current Wasm instance.
    fn ret(
        &mut self,
        value_ptr: u32,
        value_size: usize,
        scoped_instrumenter: &mut ScopedInstrumenter,
    ) -> Error {
        const UREF_COUNT: &str = "uref_count";
        self.host_buffer = None;
        let mem_get = self
            .memory()
            .get(value_ptr, value_size)
            .map_err(|e| Error::Interpreter(e.into()));
        match mem_get {
            Ok(buf) => {
                // Set the result field in the runtime and return the proper element of the `Error`
                // enum indicating that the reason for exiting the module was a call to ret.
                self.host_buffer = bytesrepr::deserialize(buf).ok();

                let urefs = match &self.host_buffer {
                    Some(buf) => extract_urefs(buf),
                    None => Ok(vec![]),
                };
                match urefs {
                    Ok(urefs) => {
                        scoped_instrumenter.add_property(UREF_COUNT, urefs.len());
                        Error::Ret(urefs).into()
                    }
                    Err(e) => {
                        scoped_instrumenter.add_property(UREF_COUNT, 0);
                        e.into()
                    }
                }
            }
            Err(e) => {
                scoped_instrumenter.add_property(UREF_COUNT, 0);
                e.into()
            }
        }
    }

    /// Checks if current context is the mint system contract.
    pub(crate) fn is_mint(&self, key: Key) -> bool {
        let hash = match self.context.get_system_contract(MINT) {
            Ok(hash) => hash,
            Err(_) => {
                error!("Failed to get system mint contract hash");
                return false;
            }
        };
        key.into_hash() == Some(hash.value())
    }

    /// Checks if current context is the `handle_payment` system contract.
    pub(crate) fn is_handle_payment(&self, key: Key) -> bool {
        let hash = match self.context.get_system_contract(HANDLE_PAYMENT) {
            Ok(hash) => hash,
            Err(_) => {
                error!("Failed to get system handle payment contract hash");
                return false;
            }
        };
        key.into_hash() == Some(hash.value())
    }

    /// Checks if current context is the auction system contract.
    pub(crate) fn is_auction(&self, key: Key) -> bool {
        let hash = match self.context.get_system_contract(AUCTION) {
            Ok(hash) => hash,
            Err(_) => {
                error!("Failed to get system auction contract hash");
                return false;
            }
        };
        key.into_hash() == Some(hash.value())
    }

    fn get_named_argument<T: FromBytes + CLTyped>(
        args: &RuntimeArgs,
        name: &str,
    ) -> Result<T, Error> {
        let arg: CLValue = args
            .get(name)
            .cloned()
            .ok_or(Error::Revert(ApiError::MissingArgument))?;
        arg.into_t()
            .map_err(|_| Error::Revert(ApiError::InvalidArgument))
    }

    fn reverter<T: Into<ApiError>>(error: T) -> Error {
        let api_error: ApiError = error.into();
        // NOTE: This is special casing needed to keep the native system contracts propagate
        // GasLimit properly to the user. Once support for wasm system contract will be dropped this
        // won't be necessary anymore.
        match api_error {
            ApiError::Mint(mint_error) if mint_error == mint::Error::GasLimit as u8 => {
                Error::GasLimit
            }
            ApiError::AuctionError(auction_error)
                if auction_error == auction::Error::GasLimit as u8 =>
            {
                Error::GasLimit
            }
            ApiError::HandlePayment(handle_payment_error)
                if handle_payment_error == handle_payment::Error::GasLimit as u8 =>
            {
                Error::GasLimit
            }
            api_error => Error::Revert(api_error),
        }
    }

    /// Calls host mint contract.
    pub fn call_host_mint(
        &mut self,
        protocol_version: ProtocolVersion,
        entry_point_name: &str,
        named_keys: &mut NamedKeys,
        runtime_args: &RuntimeArgs,
        extra_keys: &[Key],
        call_stack: Vec<CallStackElement>,
    ) -> Result<CLValue, Error> {
        let access_rights = {
            let mut keys: Vec<Key> = named_keys.values().cloned().collect();
            keys.extend(extra_keys);
            keys.push(self.get_mint_contract()?.into());
            keys.push(self.get_handle_payment_contract()?.into());
            extract_access_rights_from_keys(keys)
        };
        let authorization_keys = self.context.authorization_keys().to_owned();
        let account = self.context.account();
        let base_key = self.context.get_system_contract(MINT)?.into();
        let blocktime = self.context.get_blocktime();
        let deploy_hash = self.context.get_deploy_hash();
        let gas_limit = self.context.gas_limit();
        let gas_counter = self.context.gas_counter();
        let hash_address_generator = self.context.hash_address_generator();
        let uref_address_generator = self.context.uref_address_generator();
        let transfer_address_generator = self.context.transfer_address_generator();
        let correlation_id = self.context.correlation_id();
        let phase = self.context.phase();
        let transfers = self.context.transfers().to_owned();

        let mint_context = RuntimeContext::new(
            self.context.state(),
            EntryPointType::Contract,
            named_keys,
            access_rights,
            runtime_args.to_owned(),
            authorization_keys,
            account,
            base_key,
            blocktime,
            deploy_hash,
            gas_limit,
            gas_counter,
            hash_address_generator,
            uref_address_generator,
            transfer_address_generator,
            protocol_version,
            correlation_id,
            phase,
            self.config,
            transfers,
        );

        let mut mint_runtime = Runtime::new(
            self.config,
            SystemContractCache::clone(&self.system_contract_cache),
            self.module.clone(),
            self.instance.clone(),
            mint_context,
            call_stack,
            self.wasm_engine,
        );

        let system_config = self.config.system_config();
        let mint_costs = system_config.mint_costs();

        let result = match entry_point_name {
            // Type: `fn mint(amount: U512) -> Result<URef, Error>`
            mint::METHOD_MINT => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.mint)?;

                let amount: U512 = Self::get_named_argument(runtime_args, mint::ARG_AMOUNT)?;
                let result: Result<URef, mint::Error> = mint_runtime.mint(amount);
                if let Err(mint::Error::GasLimit) = result {
                    return Err(execution::Error::GasLimit);
                }
                CLValue::from_t(result).map_err(Self::reverter)
            })(),
            mint::METHOD_REDUCE_TOTAL_SUPPLY => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.reduce_total_supply)?;

                let amount: U512 = Self::get_named_argument(runtime_args, mint::ARG_AMOUNT)?;
                let result: Result<(), mint::Error> = mint_runtime.reduce_total_supply(amount);
                CLValue::from_t(result).map_err(Self::reverter)
            })(),
            // Type: `fn create() -> URef`
            mint::METHOD_CREATE => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.create)?;

                let uref = mint_runtime.mint(U512::zero()).map_err(Self::reverter)?;
                CLValue::from_t(uref).map_err(Self::reverter)
            })(),
            // Type: `fn balance(purse: URef) -> Option<U512>`
            mint::METHOD_BALANCE => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.balance)?;

                let uref: URef = Self::get_named_argument(runtime_args, mint::ARG_PURSE)?;
                let maybe_balance: Option<U512> =
                    mint_runtime.balance(uref).map_err(Self::reverter)?;
                CLValue::from_t(maybe_balance).map_err(Self::reverter)
            })(),
            // Type: `fn transfer(maybe_to: Option<AccountHash>, source: URef, target: URef, amount:
            // U512, id: Option<u64>) -> Result<(), Error>`
            mint::METHOD_TRANSFER => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.transfer)?;

                let maybe_to: Option<AccountHash> =
                    Self::get_named_argument(runtime_args, mint::ARG_TO)?;
                let source: URef = Self::get_named_argument(runtime_args, mint::ARG_SOURCE)?;
                let target: URef = Self::get_named_argument(runtime_args, mint::ARG_TARGET)?;
                let amount: U512 = Self::get_named_argument(runtime_args, mint::ARG_AMOUNT)?;
                let id: Option<u64> = Self::get_named_argument(runtime_args, mint::ARG_ID)?;
                let result: Result<(), mint::Error> =
                    mint_runtime.transfer(maybe_to, source, target, amount, id);
                CLValue::from_t(result).map_err(Self::reverter)
            })(),
            // Type: `fn read_base_round_reward() -> Result<U512, Error>`
            mint::METHOD_READ_BASE_ROUND_REWARD => (|| {
                mint_runtime.charge_system_contract_call(mint_costs.read_base_round_reward)?;

                let result: U512 = mint_runtime
                    .read_base_round_reward()
                    .map_err(Self::reverter)?;
                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            _ => CLValue::from_t(()).map_err(Self::reverter),
        };

        // Charge just for the amount that particular entry point cost - using gas cost from the
        // isolated runtime might have a recursive costs whenever system contract calls other system
        // contract.
        self.gas(mint_runtime.gas_counter() - gas_counter)?;

        // Result still contains a result, but the entrypoints logic does not exit early on errors.
        let ret = result?;

        let urefs = extract_urefs(&ret)?;
        let access_rights = extract_access_rights_from_urefs(urefs);
        self.context.access_rights_extend(access_rights);
        {
            let transfers = self.context.transfers_mut();
            *transfers = mint_runtime.context.transfers().to_owned();
        }
        Ok(ret)
    }

    /// Calls host `handle_payment` contract.
    pub fn call_host_handle_payment(
        &mut self,
        protocol_version: ProtocolVersion,
        entry_point_name: &str,
        named_keys: &mut NamedKeys,
        runtime_args: &RuntimeArgs,
        extra_keys: &[Key],
        call_stack: Vec<CallStackElement>,
    ) -> Result<CLValue, Error> {
        let access_rights = {
            let mut keys: Vec<Key> = named_keys.values().cloned().collect();
            keys.extend(extra_keys);
            keys.push(self.get_mint_contract()?.into());
            keys.push(self.get_handle_payment_contract()?.into());
            extract_access_rights_from_keys(keys)
        };
        let authorization_keys = self.context.authorization_keys().to_owned();
        let account = self.context.account();
        let base_key = self.context.get_system_contract(HANDLE_PAYMENT)?.into();
        let blocktime = self.context.get_blocktime();
        let deploy_hash = self.context.get_deploy_hash();
        let gas_limit = self.context.gas_limit();
        let gas_counter = self.context.gas_counter();
        let fn_store_id = self.context.hash_address_generator();
        let address_generator = self.context.uref_address_generator();
        let transfer_address_generator = self.context.transfer_address_generator();
        let correlation_id = self.context.correlation_id();
        let phase = self.context.phase();
        let transfers = self.context.transfers().to_owned();

        let runtime_context = RuntimeContext::new(
            self.context.state(),
            EntryPointType::Contract,
            named_keys,
            access_rights,
            runtime_args.to_owned(),
            authorization_keys,
            account,
            base_key,
            blocktime,
            deploy_hash,
            gas_limit,
            gas_counter,
            fn_store_id,
            address_generator,
            transfer_address_generator,
            protocol_version,
            correlation_id,
            phase,
            self.config,
            transfers,
        );

        let mut runtime = Runtime::new(
            self.config,
            SystemContractCache::clone(&self.system_contract_cache),
            self.module.clone(),
            self.instance.clone(),
            runtime_context,
            call_stack,
            self.wasm_engine,
        );

        let system_config = self.config.system_config();
        let handle_payment_costs = system_config.handle_payment_costs();

        let result = match entry_point_name {
            handle_payment::METHOD_GET_PAYMENT_PURSE => (|| {
                runtime.charge_system_contract_call(handle_payment_costs.get_payment_purse)?;

                let rights_controlled_purse =
                    runtime.get_payment_purse().map_err(Self::reverter)?;
                CLValue::from_t(rights_controlled_purse).map_err(Self::reverter)
            })(),
            handle_payment::METHOD_SET_REFUND_PURSE => (|| {
                runtime.charge_system_contract_call(handle_payment_costs.set_refund_purse)?;

                let purse: URef =
                    Self::get_named_argument(runtime_args, handle_payment::ARG_PURSE)?;
                runtime.set_refund_purse(purse).map_err(Self::reverter)?;
                CLValue::from_t(()).map_err(Self::reverter)
            })(),
            handle_payment::METHOD_GET_REFUND_PURSE => (|| {
                runtime.charge_system_contract_call(handle_payment_costs.get_refund_purse)?;

                let maybe_purse = runtime.get_refund_purse().map_err(Self::reverter)?;
                CLValue::from_t(maybe_purse).map_err(Self::reverter)
            })(),
            handle_payment::METHOD_FINALIZE_PAYMENT => (|| {
                runtime.charge_system_contract_call(handle_payment_costs.finalize_payment)?;

                let amount_spent: U512 =
                    Self::get_named_argument(runtime_args, handle_payment::ARG_AMOUNT)?;
                let account: AccountHash =
                    Self::get_named_argument(runtime_args, handle_payment::ARG_ACCOUNT)?;
                let target: URef =
                    Self::get_named_argument(runtime_args, handle_payment::ARG_TARGET)?;
                runtime
                    .finalize_payment(amount_spent, account, target)
                    .map_err(Self::reverter)?;
                CLValue::from_t(()).map_err(Self::reverter)
            })(),
            _ => CLValue::from_t(()).map_err(Self::reverter),
        };

        self.gas(runtime.gas_counter() - gas_counter)?;

        let ret = result?;
        let urefs = extract_urefs(&ret)?;
        let access_rights = extract_access_rights_from_urefs(urefs);
        self.context.access_rights_extend(access_rights);
        {
            let transfers = self.context.transfers_mut();
            *transfers = runtime.context.transfers().to_owned();
        }
        Ok(ret)
    }

    /// Calls host standard payment contract.
    pub fn call_host_standard_payment(&mut self) -> Result<(), Error> {
        // NOTE: This method (unlike other call_host_* methods) already runs on its own runtime
        // context.
        let gas_counter = self.gas_counter();
        let amount: U512 =
            Self::get_named_argument(self.context.args(), standard_payment::ARG_AMOUNT)?;
        let result = self.pay(amount).map_err(Self::reverter);
        self.set_gas_counter(gas_counter);
        result
    }

    /// Calls host auction contract.
    pub fn call_host_auction(
        &mut self,
        protocol_version: ProtocolVersion,
        entry_point_name: &str,
        named_keys: &mut NamedKeys,
        runtime_args: &RuntimeArgs,
        extra_keys: &[Key],
        call_stack: Vec<CallStackElement>,
    ) -> Result<CLValue, Error> {
        let access_rights = {
            let mut keys: Vec<Key> = named_keys.values().cloned().collect();
            keys.extend(extra_keys);
            keys.push(self.get_mint_contract()?.into());
            keys.push(self.get_handle_payment_contract()?.into());
            extract_access_rights_from_keys(keys)
        };
        let authorization_keys = self.context.authorization_keys().to_owned();
        let account = self.context.account();
        let base_key = self.context.get_system_contract(AUCTION)?.into();
        let blocktime = self.context.get_blocktime();
        let deploy_hash = self.context.get_deploy_hash();
        let gas_limit = self.context.gas_limit();
        let gas_counter = self.context.gas_counter();
        let fn_store_id = self.context.hash_address_generator();
        let address_generator = self.context.uref_address_generator();
        let transfer_address_generator = self.context.transfer_address_generator();
        let correlation_id = self.context.correlation_id();
        let phase = self.context.phase();

        let transfers = self.context.transfers().to_owned();

        let runtime_context = RuntimeContext::new(
            self.context.state(),
            EntryPointType::Contract,
            named_keys,
            access_rights,
            runtime_args.to_owned(),
            authorization_keys,
            account,
            base_key,
            blocktime,
            deploy_hash,
            gas_limit,
            gas_counter,
            fn_store_id,
            address_generator,
            transfer_address_generator,
            protocol_version,
            correlation_id,
            phase,
            self.config,
            transfers,
        );

        let mut runtime = Runtime::new(
            self.config,
            SystemContractCache::clone(&self.system_contract_cache),
            self.module.clone(),
            self.instance.clone(),
            runtime_context,
            call_stack,
            self.wasm_engine,
        );

        let system_config = self.config.system_config();
        let auction_costs = system_config.auction_costs();

        let result = match entry_point_name {
            auction::METHOD_GET_ERA_VALIDATORS => (|| {
                runtime.charge_system_contract_call(auction_costs.get_era_validators)?;

                let result = runtime.get_era_validators().map_err(Self::reverter)?;

                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_ADD_BID => (|| {
                runtime.charge_system_contract_call(auction_costs.add_bid)?;

                let account_hash = Self::get_named_argument(runtime_args, auction::ARG_PUBLIC_KEY)?;
                let delegation_rate =
                    Self::get_named_argument(runtime_args, auction::ARG_DELEGATION_RATE)?;
                let amount = Self::get_named_argument(runtime_args, auction::ARG_AMOUNT)?;

                let result = runtime
                    .add_bid(account_hash, delegation_rate, amount)
                    .map_err(Self::reverter)?;

                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_WITHDRAW_BID => (|| {
                runtime.charge_system_contract_call(auction_costs.withdraw_bid)?;

                let account_hash = Self::get_named_argument(runtime_args, auction::ARG_PUBLIC_KEY)?;
                let amount = Self::get_named_argument(runtime_args, auction::ARG_AMOUNT)?;

                let result = runtime
                    .withdraw_bid(account_hash, amount)
                    .map_err(Self::reverter)?;
                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_DELEGATE => (|| {
                runtime.charge_system_contract_call(auction_costs.delegate)?;

                let delegator = Self::get_named_argument(runtime_args, auction::ARG_DELEGATOR)?;
                let validator = Self::get_named_argument(runtime_args, auction::ARG_VALIDATOR)?;
                let amount = Self::get_named_argument(runtime_args, auction::ARG_AMOUNT)?;

                let result = runtime
                    .delegate(delegator, validator, amount)
                    .map_err(Self::reverter)?;

                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_UNDELEGATE => (|| {
                runtime.charge_system_contract_call(auction_costs.undelegate)?;

                let delegator = Self::get_named_argument(runtime_args, auction::ARG_DELEGATOR)?;
                let validator = Self::get_named_argument(runtime_args, auction::ARG_VALIDATOR)?;
                let amount = Self::get_named_argument(runtime_args, auction::ARG_AMOUNT)?;

                let result = runtime
                    .undelegate(delegator, validator, amount)
                    .map_err(Self::reverter)?;

                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_RUN_AUCTION => (|| {
                runtime.charge_system_contract_call(auction_costs.run_auction)?;

                let era_end_timestamp_millis =
                    Self::get_named_argument(runtime_args, auction::ARG_ERA_END_TIMESTAMP_MILLIS)?;
                let evicted_validators =
                    Self::get_named_argument(runtime_args, auction::ARG_EVICTED_VALIDATORS)?;

                runtime
                    .run_auction(era_end_timestamp_millis, evicted_validators)
                    .map_err(Self::reverter)?;

                CLValue::from_t(()).map_err(Self::reverter)
            })(),

            // Type: `fn slash(validator_account_hashes: &[AccountHash]) -> Result<(), Error>`
            auction::METHOD_SLASH => (|| {
                runtime.charge_system_contract_call(auction_costs.slash)?;

                let validator_public_keys =
                    Self::get_named_argument(runtime_args, auction::ARG_VALIDATOR_PUBLIC_KEYS)?;
                runtime
                    .slash(validator_public_keys)
                    .map_err(Self::reverter)?;
                CLValue::from_t(()).map_err(Self::reverter)
            })(),

            // Type: `fn distribute(reward_factors: BTreeMap<PublicKey, u64>) -> Result<(), Error>`
            auction::METHOD_DISTRIBUTE => (|| {
                runtime.charge_system_contract_call(auction_costs.distribute)?;

                let reward_factors: BTreeMap<PublicKey, u64> =
                    Self::get_named_argument(runtime_args, auction::ARG_REWARD_FACTORS)?;
                runtime.distribute(reward_factors).map_err(Self::reverter)?;
                CLValue::from_t(()).map_err(Self::reverter)
            })(),

            // Type: `fn read_era_id() -> Result<EraId, Error>`
            auction::METHOD_READ_ERA_ID => (|| {
                runtime.charge_system_contract_call(auction_costs.read_era_id)?;

                let result = runtime.read_era_id().map_err(Self::reverter)?;
                CLValue::from_t(result).map_err(Self::reverter)
            })(),

            auction::METHOD_ACTIVATE_BID => (|| {
                runtime.charge_system_contract_call(auction_costs.read_era_id)?;

                let validator_public_key: PublicKey =
                    Self::get_named_argument(runtime_args, auction::ARG_VALIDATOR_PUBLIC_KEY)?;

                runtime
                    .activate_bid(validator_public_key)
                    .map_err(Self::reverter)?;

                CLValue::from_t(()).map_err(Self::reverter)
            })(),

            _ => CLValue::from_t(()).map_err(Self::reverter),
        };

        // Charge for the gas spent during execution in an isolated runtime.
        self.gas(runtime.gas_counter() - gas_counter)?;

        // Result still contains a result, but the entrypoints logic does not exit early on errors.
        let ret = result?;

        let urefs = extract_urefs(&ret)?;
        let access_rights = extract_access_rights_from_urefs(urefs);
        self.context.access_rights_extend(access_rights);
        {
            let transfers = self.context.transfers_mut();
            *transfers = runtime.context.transfers().to_owned();
        }

        Ok(ret)
    }

    /// Calls contract living under a `key`, with supplied `args`.
    pub fn call_contract(
        &mut self,
        contract_hash: ContractHash,
        entry_point_name: &str,
        args: RuntimeArgs,
    ) -> Result<CLValue, Error> {
        let key = contract_hash.into();
        let contract = match self.context.read_gs(&key)? {
            Some(StoredValue::Contract(contract)) => contract,
            Some(_) => {
                return Err(Error::InvalidContract(contract_hash));
            }
            None => return Err(Error::KeyNotFound(key)),
        };

        let contract_entry_point = contract
            .entry_point(entry_point_name)
            .cloned()
            .ok_or_else(|| Error::NoSuchMethod(entry_point_name.to_owned()))?;

        let contract_package_hash = contract.contract_package_hash();

        let contract_package = match self.context.read_gs(&contract_package_hash.into())? {
            Some(StoredValue::ContractPackage(contract_package)) => contract_package,
            Some(_) => {
                return Err(Error::InvalidContractPackage(contract_package_hash));
            }
            None => return Err(Error::KeyNotFound(key)),
        };

        self.call_contract_checked(
            contract_package,
            contract_hash,
            contract,
            contract_entry_point,
            args,
        )
    }

    /// Calls `version` of the contract living at `key`, invoking `method` with
    /// supplied `args`. This function also checks the args conform with the
    /// types given in the contract header.
    pub fn call_versioned_contract(
        &mut self,
        contract_package_hash: ContractPackageHash,
        contract_version: Option<ContractVersion>,
        entry_point_name: String,
        args: RuntimeArgs,
    ) -> Result<CLValue, Error> {
        let contract_package_key = contract_package_hash.into();
        let contract_package = match self.context.read_gs(&contract_package_key)? {
            Some(StoredValue::ContractPackage(contract_package)) => contract_package,
            Some(_) => {
                return Err(Error::InvalidContractPackage(contract_package_hash));
            }
            None => return Err(Error::KeyNotFound(contract_package_key)),
        };

        let contract_version_key = match contract_version {
            Some(version) => {
                ContractVersionKey::new(self.context.protocol_version().value().major, version)
            }
            None => match contract_package.current_contract_version() {
                Some(v) => v,
                None => return Err(Error::NoActiveContractVersions(contract_package_hash)),
            },
        };

        // Get contract entry point hash
        let contract_hash = contract_package
            .lookup_contract_hash(contract_version_key)
            .cloned()
            .ok_or(Error::InvalidContractVersion(contract_version_key))?;

        // Get contract data
        let contract_key = contract_hash.into();
        let contract = match self.context.read_gs(&contract_key)? {
            Some(StoredValue::Contract(contract)) => contract,
            Some(_) => {
                return Err(Error::InvalidContract(contract_hash));
            }
            None => return Err(Error::KeyNotFound(contract_key)),
        };

        let contract_entry_point = contract
            .entry_point(&entry_point_name)
            .cloned()
            .ok_or_else(|| Error::NoSuchMethod(entry_point_name.to_owned()))?;

        self.call_contract_checked(
            contract_package,
            contract_hash,
            contract,
            contract_entry_point,
            args,
        )
    }

    /// Calls contract if caller has access, and args match entry point definition
    fn call_contract_checked(
        &mut self,
        contract_package: ContractPackage,
        contract_hash: ContractHash,
        contract: Contract,
        entry_point: EntryPoint,
        args: RuntimeArgs,
    ) -> Result<CLValue, Error> {
        // if public, allowed
        // if not public, restricted to user group access
        self.validate_entry_point_access(&contract_package, entry_point.access())?;

        // This will skip arguments check for system contracts only. This code should be removed on
        // next major version bump. Argument checks for system contract is still done during
        // execution of a system contract.
        if !self
            .context
            .system_contract_registry()?
            .values()
            .any(|&system_hash| system_hash == contract_hash)
        {
            let entry_point_args_lookup: BTreeMap<&str, &Parameter> = entry_point
                .args()
                .iter()
                .map(|param| {
                    // Skips all optional parameters
                    (param.name(), param)
                })
                .collect();

            let args_lookup: BTreeMap<&str, &NamedArg> = args
                .named_args()
                .map(|named_arg| (named_arg.name(), named_arg))
                .collect();

            // ensure args type(s) match defined args of entry point

            for (param_name, param) in entry_point_args_lookup {
                if let Some(named_arg) = args_lookup.get(param_name) {
                    if param.cl_type() != named_arg.cl_value().cl_type() {
                        return Err(Error::type_mismatch(
                            param.cl_type().clone(),
                            named_arg.cl_value().cl_type().clone(),
                        ));
                    }
                } else if !param.cl_type().is_option() {
                    return Err(Error::MissingArgument {
                        name: param.name().to_string(),
                    });
                }
            }
        }

        // if session the caller's context
        // else the called contract's context
        let context_key = self.get_context_key_for_contract_call(contract_hash, &entry_point)?;

        self.execute_contract(
            context_key,
            context_key,
            contract_hash,
            contract,
            args,
            entry_point,
            self.context.protocol_version(),
        )
    }

    fn get_context_key_for_contract_call(
        &self,
        contract_hash: ContractHash,
        entry_point: &EntryPoint,
    ) -> Result<Key, Error> {
        let current = self.context.entry_point_type();
        let next = entry_point.entry_point_type();
        match (current, next) {
            (EntryPointType::Contract, EntryPointType::Session) => {
                // Session code can't be called from Contract code for security reasons.
                Err(Error::InvalidContext)
            }
            (EntryPointType::Session, EntryPointType::Session) => {
                // Session code called from session reuses current base key
                Ok(self.context.base_key())
            }
            (EntryPointType::Session, EntryPointType::Contract)
            | (EntryPointType::Contract, EntryPointType::Contract) => Ok(contract_hash.into()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_contract(
        &mut self,
        key: Key,
        base_key: Key,
        contract_hash: ContractHash,
        contract: Contract,
        args: RuntimeArgs,
        entry_point: EntryPoint,
        protocol_version: ProtocolVersion,
    ) -> Result<CLValue, Error> {
        // Check for major version compatibility before calling
        if !contract.is_compatible_protocol_version(protocol_version) {
            return Err(Error::IncompatibleProtocolMajorVersion {
                actual: contract.protocol_version().value().major,
                expected: protocol_version.value().major,
            });
        }

        // TODO: should we be using named_keys_mut() instead?
        let mut named_keys = match entry_point.entry_point_type() {
            EntryPointType::Session => self.context.account().named_keys().clone(),
            EntryPointType::Contract => contract.named_keys().clone(),
        };

        let extra_keys = {
            let mut extra_keys = vec![];
            // A loop is needed to be able to use the '?' operator
            for arg in args.to_values() {
                extra_keys.extend(
                    extract_urefs(arg)?
                        .into_iter()
                        .map(<Key as From<URef>>::from),
                );
            }
            for key in &extra_keys {
                self.context.validate_key(key)?;
            }

            if self.is_mint(key) {
                let mut call_stack = self.call_stack.to_owned();
                let call_stack_element = CallStackElement::stored_contract(
                    contract.contract_package_hash(),
                    contract_hash,
                );
                call_stack.push(call_stack_element);

                return self.call_host_mint(
                    self.context.protocol_version(),
                    entry_point.name(),
                    &mut named_keys,
                    &args,
                    &extra_keys,
                    call_stack,
                );
            } else if self.is_handle_payment(key) {
                let mut call_stack = self.call_stack.to_owned();
                let call_stack_element = CallStackElement::stored_contract(
                    contract.contract_package_hash(),
                    contract_hash,
                );
                call_stack.push(call_stack_element);

                return self.call_host_handle_payment(
                    self.context.protocol_version(),
                    entry_point.name(),
                    &mut named_keys,
                    &args,
                    &extra_keys,
                    call_stack,
                );
            } else if self.is_auction(key) {
                let mut call_stack = self.call_stack.to_owned();
                let call_stack_element = CallStackElement::stored_contract(
                    contract.contract_package_hash(),
                    contract_hash,
                );
                call_stack.push(call_stack_element);

                return self.call_host_auction(
                    self.context.protocol_version(),
                    entry_point.name(),
                    &mut named_keys,
                    &args,
                    &extra_keys,
                    call_stack,
                );
            }

            extra_keys
        };

        let module = {
            let maybe_module = key
                .into_hash()
                .and_then(|hash_addr| self.system_contract_cache.get(hash_addr.into()));
            let wasm_key = contract.contract_wasm_key();

            let contract_wasm: ContractWasm = match self.context.read_gs(&wasm_key)? {
                Some(StoredValue::ContractWasm(contract_wasm)) => contract_wasm,
                Some(_) => return Err(Error::InvalidContractWasm(contract.contract_wasm_hash())),
                None => return Err(Error::KeyNotFound(key)),
            };
            match maybe_module {
                Some(module) => module,
                None => self.wasm_engine.module_from_bytes(contract_wasm.bytes())?,
            }
        };

        let entry_point_name = entry_point.name();

        let instance = instance_and_memory(module.clone(), protocol_version, self.wasm_engine)?;

        let access_rights = {
            let mut keys: Vec<Key> = named_keys.values().cloned().collect();
            keys.extend(extra_keys);
            keys.push(self.get_mint_contract()?.into());
            keys.push(self.get_handle_payment_contract()?.into());
            extract_access_rights_from_keys(keys)
        };

        let system_contract_cache = SystemContractCache::clone(&self.system_contract_cache);

        let config = self.config;

        let host_buffer = None;

        let context = RuntimeContext::new(
            self.context.state(),
            entry_point.entry_point_type(),
            &mut named_keys,
            access_rights,
            args,
            self.context.authorization_keys().clone(),
            self.context.account(),
            base_key,
            self.context.get_blocktime(),
            self.context.get_deploy_hash(),
            self.context.gas_limit(),
            self.context.gas_counter(),
            self.context.hash_address_generator(),
            self.context.uref_address_generator(),
            self.context.transfer_address_generator(),
            protocol_version,
            self.context.correlation_id(),
            self.context.phase(),
            self.config,
            self.context.transfers().to_owned(),
        );

        let mut call_stack = self.call_stack.to_owned();

        let call_stack_element = match entry_point.entry_point_type() {
            EntryPointType::Session => CallStackElement::stored_session(
                self.context.account().account_hash(),
                contract.contract_package_hash(),
                contract_hash,
            ),
            EntryPointType::Contract => {
                CallStackElement::stored_contract(contract.contract_package_hash(), contract_hash)
            }
        };

        call_stack.push(call_stack_element);

        let mut runtime = Runtime {
            system_contract_cache,
            config,
            module,
            instance,
            host_buffer,
            context,
            call_stack,
            wasm_engine: self.wasm_engine,
        };

        let instance_ref = runtime.instance().clone();

        let result = instance_ref.invoke_export(entry_point_name, Vec::new(), &mut runtime);

        // The `runtime`'s context was initialized with our counter from before the call and any gas
        // charged by the sub-call was added to its counter - so let's copy the correct value of the
        // counter from there to our counter
        self.context.set_gas_counter(runtime.context.gas_counter());

        {
            let transfers = self.context.transfers_mut();
            *transfers = runtime.context.transfers().to_owned();
        }

        let error = match result {
            Err(error) => error,
            // If `Ok` and the `host_buffer` is `None`, the contract's execution succeeded but did
            // not explicitly call `runtime::ret()`.  Treat as though the execution
            // returned the unit type `()` as per Rust functions which don't specify a
            // return value.
            Ok(_) => {
                if self.context.entry_point_type() == EntryPointType::Session
                    && runtime.context.entry_point_type() == EntryPointType::Session
                {
                    // Overwrites parent's named keys with child's new named key but only when
                    // running session code
                    *self.context.named_keys_mut() = runtime.context.named_keys().clone();
                }
                return Ok(runtime.take_host_buffer().unwrap_or(CLValue::from_t(())?));
            }
        };

        if let Some(host_error) = error.as_execution_error() {
            // If the "error" was in fact a trap caused by calling `ret` then
            // this is normal operation and we should return the value captured
            // in the Runtime result field.
            match host_error {
                Error::Ret(ref ret_urefs) => {
                    // insert extra urefs returned from call
                    let ret_urefs_map: HashMap<Address, HashSet<AccessRights>> =
                        extract_access_rights_from_urefs(ret_urefs.clone());
                    self.context.access_rights_extend(ret_urefs_map);
                    // if ret has not set host_buffer consider it programmer error
                    if self.context.entry_point_type() == EntryPointType::Session
                        && runtime.context.entry_point_type() == EntryPointType::Session
                    {
                        // Overwrites parent's named keys with child's new named key but only when
                        // running session code
                        *self.context.named_keys_mut() = runtime.context.named_keys().clone();
                    }
                    return runtime.take_host_buffer().ok_or(Error::ExpectedReturnValue);
                }
                error => return Err(error.clone()),
                // None => return Err(Error::Interpreter(host_error.to_string())),
            }
        } else {
            Err(Error::Interpreter(error.to_string()))
        }
    }

    fn call_contract_host_buffer(
        &mut self,
        contract_hash: ContractHash,
        entry_point_name: &str,
        args_bytes: Vec<u8>,
        result_size_ptr: u32,
        scoped_instrumenter: &mut ScopedInstrumenter,
    ) -> Result<Result<(), ApiError>, Error> {
        // Exit early if the host buffer is already occupied
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }
        let args: RuntimeArgs = bytesrepr::deserialize(args_bytes)?;
        scoped_instrumenter.pause();
        let result = self.call_contract(contract_hash, entry_point_name, args)?;
        scoped_instrumenter.unpause();
        self.manage_call_contract_host_buffer(result_size_ptr, result)
    }

    fn call_versioned_contract_host_buffer(
        &mut self,
        contract_package_hash: ContractPackageHash,
        contract_version: Option<ContractVersion>,
        entry_point_name: String,
        args_bytes: Vec<u8>,
        result_size_ptr: u32,
        scoped_instrumenter: &mut ScopedInstrumenter,
    ) -> Result<Result<(), ApiError>, Error> {
        // Exit early if the host buffer is already occupied
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }
        let args: RuntimeArgs = bytesrepr::deserialize(args_bytes)?;
        scoped_instrumenter.pause();
        let result = self.call_versioned_contract(
            contract_package_hash,
            contract_version,
            entry_point_name,
            args,
        )?;
        scoped_instrumenter.unpause();
        self.manage_call_contract_host_buffer(result_size_ptr, result)
    }

    fn check_host_buffer(&mut self) -> Result<(), ApiError> {
        if !self.can_write_to_host_buffer() {
            Err(ApiError::HostBufferFull)
        } else {
            Ok(())
        }
    }

    fn manage_call_contract_host_buffer(
        &mut self,
        result_size_ptr: u32,
        result: CLValue,
    ) -> Result<Result<(), ApiError>, Error> {
        let result_size = result.inner_bytes().len() as u32; // considered to be safe

        // leave the host buffer set to `None` if there's nothing to write there
        if result_size != 0 {
            if let Err(error) = self.write_host_buffer(result) {
                return Ok(Err(error));
            }
        }

        let result_size_bytes = result_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(result_size_ptr, &result_size_bytes) {
            return Err(Error::Interpreter(error.into()));
        }

        Ok(Ok(()))
    }

    fn load_named_keys(
        &mut self,
        total_keys_ptr: u32,
        result_size_ptr: u32,
        scoped_instrumenter: &mut ScopedInstrumenter,
    ) -> Result<Result<(), ApiError>, Error> {
        scoped_instrumenter.add_property(
            "names_total_length",
            self.context
                .named_keys()
                .keys()
                .map(|name| name.len())
                .sum::<usize>(),
        );

        if !self.can_write_to_host_buffer() {
            // Exit early if the host buffer is already occupied
            return Ok(Err(ApiError::HostBufferFull));
        }

        let total_keys = self.context.named_keys().len() as u32;
        let total_keys_bytes = total_keys.to_le_bytes();
        if let Err(error) = self.memory().set(total_keys_ptr, &total_keys_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        if total_keys == 0 {
            // No need to do anything else, we leave host buffer empty.
            return Ok(Ok(()));
        }

        let named_keys =
            CLValue::from_t(self.context.named_keys().clone()).map_err(Error::CLValue)?;

        let length = named_keys.inner_bytes().len() as u32;
        if let Err(error) = self.write_host_buffer(named_keys) {
            return Ok(Err(error));
        }

        let length_bytes = length.to_le_bytes();
        if let Err(error) = self.memory().set(result_size_ptr, &length_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        Ok(Ok(()))
    }

    fn create_contract_package(
        &mut self,
        is_locked: ContractPackageStatus,
    ) -> Result<(ContractPackage, URef), Error> {
        let access_key = self.context.new_unit_uref()?;
        let contract_package = ContractPackage::new(
            access_key,
            ContractVersions::default(),
            DisabledVersions::default(),
            Groups::default(),
            is_locked,
        );

        Ok((contract_package, access_key))
    }

    fn create_contract_package_at_hash(
        &mut self,
        lock_status: ContractPackageStatus,
    ) -> Result<([u8; 32], [u8; 32]), Error> {
        let addr = self.context.new_hash_address()?;
        let (contract_package, access_key) = self.create_contract_package(lock_status)?;
        self.context
            .metered_write_gs_unsafe(Key::Hash(addr), contract_package)?;
        Ok((addr, access_key.addr()))
    }

    fn create_contract_user_group(
        &mut self,
        contract_package_hash: ContractPackageHash,
        label: String,
        num_new_urefs: u32,
        mut existing_urefs: BTreeSet<URef>,
        output_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let mut contract_package: ContractPackage = self
            .context
            .get_validated_contract_package(contract_package_hash)?;

        let groups = contract_package.groups_mut();
        let new_group = Group::new(label);

        // Ensure group does not already exist
        if groups.get(&new_group).is_some() {
            return Ok(Err(contracts::Error::GroupAlreadyExists.into()));
        }

        // Ensure there are not too many groups
        if groups.len() >= (contracts::MAX_GROUPS as usize) {
            return Ok(Err(contracts::Error::MaxGroupsExceeded.into()));
        }

        // Ensure there are not too many urefs
        let total_urefs: usize = groups.values().map(|urefs| urefs.len()).sum::<usize>()
            + (num_new_urefs as usize)
            + existing_urefs.len();
        if total_urefs > contracts::MAX_TOTAL_UREFS {
            let err = contracts::Error::MaxTotalURefsExceeded;
            return Ok(Err(ApiError::ContractHeader(err as u8)));
        }

        // Proceed with creating user group
        let mut new_urefs = Vec::with_capacity(num_new_urefs as usize);
        for _ in 0..num_new_urefs {
            let u = self.context.new_unit_uref()?;
            new_urefs.push(u);
        }

        for u in new_urefs.iter().cloned() {
            existing_urefs.insert(u);
        }
        groups.insert(new_group, existing_urefs);

        // check we can write to the host buffer
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }
        // create CLValue for return value
        let new_urefs_value = CLValue::from_t(new_urefs)?;
        let value_size = new_urefs_value.inner_bytes().len();
        // write return value to buffer
        if let Err(err) = self.write_host_buffer(new_urefs_value) {
            return Ok(Err(err));
        }
        // Write return value size to output location
        let output_size_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &output_size_bytes) {
            return Err(Error::Interpreter(error.into()));
        }

        // Write updated package to the global state
        self.context
            .metered_write_gs_unsafe(contract_package_hash, contract_package)?;

        Ok(Ok(()))
    }

    #[allow(clippy::too_many_arguments)]
    fn add_contract_version(
        &mut self,
        contract_package_hash: ContractPackageHash,
        entry_points: EntryPoints,
        mut named_keys: NamedKeys,
        output_ptr: u32,
        output_size: usize,
        bytes_written_ptr: u32,
        version_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        self.context
            .validate_key(&Key::from(contract_package_hash))?;

        let mut contract_package: ContractPackage = self
            .context
            .get_validated_contract_package(contract_package_hash)?;

        let version = contract_package.current_contract_version();

        // Return an error if the contract is locked and has some version associated with it.
        if contract_package.is_locked() && version.is_some() {
            return Err(Error::LockedContract(contract_package_hash));
        }

        let contract_wasm_hash = self.context.new_hash_address()?;
        let contract_wasm = {
            let module_bytes = self.get_module_from_entry_points(&entry_points)?;
            ContractWasm::new(module_bytes)
        };

        let contract_hash = self.context.new_hash_address()?;

        let protocol_version = self.context.protocol_version();
        let major = protocol_version.value().major;

        // TODO: EE-1032 - Implement different ways of carrying on existing named keys
        if let Some(previous_contract_hash) = contract_package.current_contract_hash() {
            let previous_contract: Contract =
                self.context.read_gs_typed(&previous_contract_hash.into())?;

            let mut previous_named_keys = previous_contract.take_named_keys();
            named_keys.append(&mut previous_named_keys);
        }

        let contract = Contract::new(
            contract_package_hash,
            contract_wasm_hash.into(),
            named_keys,
            entry_points,
            protocol_version,
        );

        let insert_contract_result =
            contract_package.insert_contract_version(major, contract_hash.into());

        self.context
            .metered_write_gs_unsafe(Key::Hash(contract_wasm_hash), contract_wasm)?;
        self.context
            .metered_write_gs_unsafe(Key::Hash(contract_hash), contract)?;
        self.context
            .metered_write_gs_unsafe(contract_package_hash, contract_package)?;

        // return contract key to caller
        {
            let key_bytes = match contract_hash.to_bytes() {
                Ok(bytes) => bytes,
                Err(error) => return Ok(Err(error.into())),
            };

            // `output_size` must be >= actual length of serialized Key bytes
            if output_size < key_bytes.len() {
                return Ok(Err(ApiError::BufferTooSmall));
            }

            // Set serialized Key bytes into the output buffer
            if let Err(error) = self.memory().set(output_ptr, &key_bytes) {
                return Err(Error::Interpreter(error.into()));
            }

            // Following cast is assumed to be safe
            let bytes_size = key_bytes.len() as u32;
            let size_bytes = bytes_size.to_le_bytes(); // Wasm is little-endian
            if let Err(error) = self.memory().set(bytes_written_ptr, &size_bytes) {
                return Err(Error::Interpreter(error.into()));
            }

            let version_value: u32 = insert_contract_result.contract_version();
            let version_bytes = version_value.to_le_bytes();
            if let Err(error) = self.memory().set(version_ptr, &version_bytes) {
                return Err(Error::Interpreter(error.into()));
            }
        }

        Ok(Ok(()))
    }

    fn disable_contract_version(
        &mut self,
        contract_package_hash: ContractPackageHash,
        contract_hash: ContractHash,
    ) -> Result<Result<(), ApiError>, Error> {
        let contract_package_key = contract_package_hash.into();
        self.context.validate_key(&contract_package_key)?;

        let mut contract_package: ContractPackage = self
            .context
            .get_validated_contract_package(contract_package_hash)?;

        // Return an error in trying to disable the (singular) version of a locked contract.
        if contract_package.is_locked() {
            return Err(Error::LockedContract(contract_package_hash));
        }

        if let Err(err) = contract_package.disable_contract_version(contract_hash) {
            return Ok(Err(err.into()));
        }

        self.context
            .metered_write_gs_unsafe(contract_package_key, contract_package)?;

        Ok(Ok(()))
    }

    /// Writes function address (`hash_bytes`) into the Wasm memory (at
    /// `dest_ptr` pointer).
    fn function_address(&mut self, hash_bytes: [u8; 32], dest_ptr: u32) -> Result<(), Error> {
        self.memory()
            .set(dest_ptr, &hash_bytes)
            .map_err(|e| Error::Interpreter(e.into()).into())
    }

    /// Generates new unforgable reference and adds it to the context's
    /// access_rights set.
    fn new_uref(&mut self, uref_ptr: u32, value_ptr: u32, value_size: u32) -> Result<(), Error> {
        let cl_value = self.cl_value_from_mem(value_ptr, value_size)?; // read initial value from memory
        let uref = self.context.new_uref(StoredValue::CLValue(cl_value))?;
        self.memory()
            .set(uref_ptr, &uref.into_bytes().map_err(Error::BytesRepr)?)
            .map_err(|e| Error::Interpreter(e.into()).into())
    }

    /// Writes `value` under `key` in GlobalState.
    fn write(
        &mut self,
        key_ptr: u32,
        key_size: u32,
        value_ptr: u32,
        value_size: u32,
    ) -> Result<(), Error> {
        let key = self.key_from_mem(key_ptr, key_size)?;
        let cl_value = self.cl_value_from_mem(value_ptr, value_size)?;
        self.context
            .metered_write_gs(key, cl_value)
            .map_err(Into::into)
    }

    /// Records a transfer.
    fn record_transfer(
        &mut self,
        maybe_to: Option<AccountHash>,
        source: URef,
        target: URef,
        amount: U512,
        id: Option<u64>,
    ) -> Result<(), Error> {
        if self.context.base_key() != Key::from(self.context.get_system_contract(MINT)?) {
            return Err(Error::InvalidContext);
        }

        if self.context.phase() != Phase::Session {
            return Ok(());
        }

        let transfer_addr = self.context.new_transfer_addr()?;
        let transfer = {
            let deploy_hash: DeployHash = self.context.get_deploy_hash();
            let from: AccountHash = self.context.account().account_hash();
            let fee: U512 = U512::zero(); // TODO
            Transfer::new(deploy_hash, from, maybe_to, source, target, amount, fee, id)
        };
        {
            let transfers = self.context.transfers_mut();
            transfers.push(transfer_addr);
        }
        self.context
            .write_transfer(Key::Transfer(transfer_addr), transfer);
        Ok(())
    }

    /// Records given auction info at a given era id
    fn record_era_info(&mut self, era_id: EraId, era_info: EraInfo) -> Result<(), Error> {
        if self.context.base_key() != Key::from(self.context.get_system_contract(AUCTION)?) {
            return Err(Error::InvalidContext);
        }

        if self.context.phase() != Phase::Session {
            return Ok(());
        }

        self.context.write_era_info(Key::EraInfo(era_id), era_info);

        Ok(())
    }

    /// Adds `value` to the cell that `key` points at.
    fn add(
        &mut self,
        key_ptr: u32,
        key_size: u32,
        value_ptr: u32,
        value_size: u32,
    ) -> Result<(), Error> {
        let key = self.key_from_mem(key_ptr, key_size)?;
        let cl_value = self.cl_value_from_mem(value_ptr, value_size)?;
        self.context
            .metered_add_gs(key, cl_value)
            .map_err(Into::into)
    }

    /// Reads value from the GS living under key specified by `key_ptr` and
    /// `key_size`. Wasm and host communicate through memory that Wasm
    /// module exports. If contract wants to pass data to the host, it has
    /// to tell it [the host] where this data lives in the exported memory
    /// (pass its pointer and length).
    fn read(
        &mut self,
        key_ptr: u32,
        key_size: u32,
        output_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        if !self.can_write_to_host_buffer() {
            // Exit early if the host buffer is already occupied
            return Ok(Err(ApiError::HostBufferFull));
        }

        let key = self.key_from_mem(key_ptr, key_size)?;
        let cl_value = match self.context.read_gs(&key)? {
            Some(stored_value) => CLValue::try_from(stored_value).map_err(Error::TypeMismatch)?,
            None => return Ok(Err(ApiError::ValueNotFound)),
        };

        let value_size = cl_value.inner_bytes().len() as u32;
        if let Err(error) = self.write_host_buffer(cl_value) {
            return Ok(Err(error));
        }

        let value_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &value_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        Ok(Ok(()))
    }

    fn add_associated_key(
        &mut self,
        account_hash_ptr: u32,
        account_hash_size: usize,
        weight_value: u8,
    ) -> Result<i32, Error> {
        let account_hash = {
            // Account hash as serialized bytes
            let source_serialized = self.bytes_from_mem(account_hash_ptr, account_hash_size)?;
            // Account hash deserialized
            let source: AccountHash =
                bytesrepr::deserialize(source_serialized).map_err(Error::BytesRepr)?;
            source
        };
        let weight = Weight::new(weight_value);

        match self.context.add_associated_key(account_hash, weight) {
            Ok(_) => Ok(0),
            // This relies on the fact that `AddKeyFailure` is represented as
            // i32 and first variant start with number `1`, so all other variants
            // are greater than the first one, so it's safe to assume `0` is success,
            // and any error is greater than 0.
            Err(Error::AddKeyFailure(e)) => Ok(e as i32),
            // Any other variant just pass as `Trap`
            Err(e) => Err(e.into()),
        }
    }

    fn remove_associated_key(
        &mut self,
        account_hash_ptr: u32,
        account_hash_size: usize,
    ) -> Result<i32, Error> {
        let account_hash = {
            // Account hash as serialized bytes
            let source_serialized = self.bytes_from_mem(account_hash_ptr, account_hash_size)?;
            // Account hash deserialized
            let source: AccountHash =
                bytesrepr::deserialize(source_serialized).map_err(Error::BytesRepr)?;
            source
        };
        match self.context.remove_associated_key(account_hash) {
            Ok(_) => Ok(0),
            Err(Error::RemoveKeyFailure(e)) => Ok(e as i32),
            Err(e) => Err(e.into()),
        }
    }

    fn update_associated_key(
        &mut self,
        account_hash_ptr: u32,
        account_hash_size: usize,
        weight_value: u8,
    ) -> Result<i32, Error> {
        let account_hash = {
            // Account hash as serialized bytes
            let source_serialized = self.bytes_from_mem(account_hash_ptr, account_hash_size)?;
            // Account hash deserialized
            let source: AccountHash =
                bytesrepr::deserialize(source_serialized).map_err(Error::BytesRepr)?;
            source
        };
        let weight = Weight::new(weight_value);

        match self.context.update_associated_key(account_hash, weight) {
            Ok(_) => Ok(0),
            // This relies on the fact that `UpdateKeyFailure` is represented as
            // i32 and first variant start with number `1`, so all other variants
            // are greater than the first one, so it's safe to assume `0` is success,
            // and any error is greater than 0.
            Err(Error::UpdateKeyFailure(e)) => Ok(e as i32),
            // Any other variant just pass as `Trap`
            Err(e) => Err(e.into()),
        }
    }

    fn set_action_threshold(
        &mut self,
        action_type_value: u32,
        threshold_value: u8,
    ) -> Result<i32, Error> {
        match ActionType::try_from(action_type_value) {
            Ok(action_type) => {
                let threshold = Weight::new(threshold_value);
                match self.context.set_action_threshold(action_type, threshold) {
                    Ok(_) => Ok(0),
                    Err(Error::SetThresholdFailure(e)) => Ok(e as i32),
                    Err(e) => Err(e.into()),
                }
            }
            Err(_) => todo!("Err(Trap::new(TrapKind::Unreachable))"),
        }
    }

    /// Looks up the public mint contract key in the context's protocol data.
    ///
    /// Returned URef is already attenuated depending on the calling account.
    fn get_mint_contract(&self) -> Result<ContractHash, Error> {
        self.context.get_system_contract(MINT)
    }

    /// Looks up the public handle payment contract key in the context's protocol data.
    ///
    /// Returned URef is already attenuated depending on the calling account.
    fn get_handle_payment_contract(&self) -> Result<ContractHash, Error> {
        self.context.get_system_contract(HANDLE_PAYMENT)
    }

    /// Looks up the public standard payment contract key in the context's protocol data.
    ///
    /// Returned URef is already attenuated depending on the calling account.
    fn get_standard_payment_contract(&self) -> Result<ContractHash, Error> {
        self.context.get_system_contract(STANDARD_PAYMENT)
    }

    /// Looks up the public auction contract key in the context's protocol data.
    ///
    /// Returned URef is already attenuated depending on the calling account.
    fn get_auction_contract(&self) -> Result<ContractHash, Error> {
        self.context.get_system_contract(AUCTION)
    }

    /// Calls the `read_base_round_reward` method on the mint contract at the given mint
    /// contract key
    fn mint_read_base_round_reward(
        &mut self,
        mint_contract_hash: ContractHash,
    ) -> Result<U512, Error> {
        let gas_counter = self.gas_counter();
        let call_result = self.call_contract(
            mint_contract_hash,
            mint::METHOD_READ_BASE_ROUND_REWARD,
            RuntimeArgs::default(),
        );
        self.set_gas_counter(gas_counter);

        let reward = call_result?.into_t()?;
        Ok(reward)
    }

    /// Calls the `mint` method on the mint contract at the given mint
    /// contract key
    fn mint_mint(&mut self, mint_contract_hash: ContractHash, amount: U512) -> Result<URef, Error> {
        let gas_counter = self.gas_counter();
        let runtime_args = {
            let mut runtime_args = RuntimeArgs::new();
            runtime_args.insert(mint::ARG_AMOUNT, amount)?;
            runtime_args
        };
        let call_result = self.call_contract(mint_contract_hash, mint::METHOD_MINT, runtime_args);
        self.set_gas_counter(gas_counter);

        let result: Result<URef, mint::Error> = call_result?.into_t()?;
        Ok(result.map_err(system::Error::from)?)
    }

    /// Calls the `reduce_total_supply` method on the mint contract at the given mint
    /// contract key
    fn mint_reduce_total_supply(
        &mut self,
        mint_contract_hash: ContractHash,
        amount: U512,
    ) -> Result<(), Error> {
        let gas_counter = self.gas_counter();
        let runtime_args = {
            let mut runtime_args = RuntimeArgs::new();
            runtime_args.insert(mint::ARG_AMOUNT, amount)?;
            runtime_args
        };
        let call_result = self.call_contract(
            mint_contract_hash,
            mint::METHOD_REDUCE_TOTAL_SUPPLY,
            runtime_args,
        );
        self.set_gas_counter(gas_counter);

        let result: Result<(), mint::Error> = call_result?.into_t()?;
        Ok(result.map_err(system::Error::from)?)
    }

    /// Calls the "create" method on the mint contract at the given mint
    /// contract key
    fn mint_create(&mut self, mint_contract_hash: ContractHash) -> Result<URef, Error> {
        let gas_counter = self.gas_counter();
        let result =
            self.call_contract(mint_contract_hash, mint::METHOD_CREATE, RuntimeArgs::new());
        self.set_gas_counter(gas_counter);

        let purse = result?.into_t()?;
        Ok(purse)
    }

    fn create_purse(&mut self) -> Result<URef, Error> {
        self.mint_create(self.get_mint_contract()?)
    }

    /// Calls the "transfer" method on the mint contract at the given mint
    /// contract key
    fn mint_transfer(
        &mut self,
        mint_contract_hash: ContractHash,
        to: Option<AccountHash>,
        source: URef,
        target: URef,
        amount: U512,
        id: Option<u64>,
    ) -> Result<Result<(), mint::Error>, Error> {
        let args_values = {
            let mut runtime_args = RuntimeArgs::new();
            runtime_args.insert(mint::ARG_TO, to)?;
            runtime_args.insert(mint::ARG_SOURCE, source)?;
            runtime_args.insert(mint::ARG_TARGET, target)?;
            runtime_args.insert(mint::ARG_AMOUNT, amount)?;
            runtime_args.insert(mint::ARG_ID, id)?;
            runtime_args
        };

        let gas_counter = self.gas_counter();
        let call_result =
            self.call_contract(mint_contract_hash, mint::METHOD_TRANSFER, args_values);
        self.set_gas_counter(gas_counter);

        Ok(call_result?.into_t()?)
    }

    /// Creates a new account at a given public key, transferring a given amount
    /// of motes from the given source purse to the new account's purse.
    fn transfer_to_new_account(
        &mut self,
        source: URef,
        target: AccountHash,
        amount: U512,
        id: Option<u64>,
    ) -> Result<TransferResult, Error> {
        let mint_contract_hash = self.get_mint_contract()?;

        let target_key = Key::Account(target);

        // A precondition check that verifies that the transfer can be done
        // as the source purse has enough funds to cover the transfer.
        if amount > self.get_balance(source)?.unwrap_or_default() {
            return Ok(Err(mint::Error::InsufficientFunds.into()));
        }

        let target_purse = self.mint_create(mint_contract_hash)?;

        if source == target_purse {
            return Ok(Err(mint::Error::EqualSourceAndTarget.into()));
        }

        match self.mint_transfer(
            mint_contract_hash,
            Some(target),
            source,
            target_purse.with_access_rights(AccessRights::ADD),
            amount,
            id,
        )? {
            Ok(()) => {
                let account = Account::create(target, Default::default(), target_purse);
                self.context.write_account(target_key, account)?;
                Ok(Ok(TransferredTo::NewAccount))
            }
            Err(mint_error) => Ok(Err(mint_error.into())),
        }
    }

    /// Transferring a given amount of motes from the given source purse to the
    /// new account's purse. Requires that the [`URef`]s have already
    /// been created by the mint contract (or are the genesis account's).
    fn transfer_to_existing_account(
        &mut self,
        to: Option<AccountHash>,
        source: URef,
        target: URef,
        amount: U512,
        id: Option<u64>,
    ) -> Result<TransferResult, Error> {
        let mint_contract_key = self.get_mint_contract()?;

        // This appears to be a load-bearing use of `RuntimeContext::insert_uref`.
        self.context.insert_uref(target);

        match self.mint_transfer(mint_contract_key, to, source, target, amount, id)? {
            Ok(()) => Ok(Ok(TransferredTo::ExistingAccount)),
            Err(error) => Ok(Err(error.into())),
        }
    }

    /// Transfers `amount` of motes from default purse of the account to
    /// `target` account. If that account does not exist, creates one.
    fn transfer_to_account(
        &mut self,
        target: AccountHash,
        amount: U512,
        id: Option<u64>,
    ) -> Result<TransferResult, Error> {
        let source = self.context.get_main_purse()?;
        self.transfer_from_purse_to_account(source, target, amount, id)
    }

    /// Transfers `amount` of motes from `source` purse to `target` account.
    /// If that account does not exist, creates one.
    fn transfer_from_purse_to_account(
        &mut self,
        source: URef,
        target: AccountHash,
        amount: U512,
        id: Option<u64>,
    ) -> Result<TransferResult, Error> {
        let target_key = Key::Account(target);
        // Look up the account at the given public key's address
        match self.context.read_account(&target_key)? {
            None => {
                // If no account exists, create a new account and transfer the amount to its
                // purse.
                self.transfer_to_new_account(source, target, amount, id)
            }
            Some(StoredValue::Account(account)) => {
                let target_uref = account.main_purse_add_only();
                if source.with_access_rights(AccessRights::ADD) == target_uref {
                    return Ok(Ok(TransferredTo::ExistingAccount));
                }
                // If an account exists, transfer the amount to its purse
                self.transfer_to_existing_account(Some(target), source, target_uref, amount, id)
            }
            Some(_) => {
                // If some other value exists, return an error
                Err(Error::AccountNotFound(target_key))
            }
        }
    }

    /// Transfers `amount` of motes from `source` purse to `target` purse.
    #[allow(clippy::too_many_arguments)]
    fn transfer_from_purse_to_purse(
        &mut self,
        source_ptr: u32,
        source_size: u32,
        target_ptr: u32,
        target_size: u32,
        amount_ptr: u32,
        amount_size: u32,
        id_ptr: u32,
        id_size: u32,
    ) -> Result<Result<(), mint::Error>, Error> {
        let source: URef = {
            let bytes = self.bytes_from_mem(source_ptr, source_size as usize)?;
            bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
        };

        let target: URef = {
            let bytes = self.bytes_from_mem(target_ptr, target_size as usize)?;
            bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
        };

        let amount: U512 = {
            let bytes = self.bytes_from_mem(amount_ptr, amount_size as usize)?;
            bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
        };

        let id: Option<u64> = {
            let bytes = self.bytes_from_mem(id_ptr, id_size as usize)?;
            bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
        };

        let mint_contract_key = self.get_mint_contract()?;

        match self.mint_transfer(mint_contract_key, None, source, target, amount, id)? {
            Ok(()) => Ok(Ok(())),
            Err(mint_error) => Ok(Err(mint_error)),
        }
    }

    fn get_balance(&mut self, purse: URef) -> Result<Option<U512>, Error> {
        let maybe_value = self.context.read_gs_direct(&Key::Balance(purse.addr()))?;
        match maybe_value {
            Some(StoredValue::CLValue(value)) => {
                let value = CLValue::into_t(value)?;
                Ok(Some(value))
            }
            Some(_) => Err(Error::UnexpectedStoredValueVariant),
            None => Ok(None),
        }
    }

    fn get_balance_host_buffer(
        &mut self,
        purse_ptr: u32,
        purse_size: usize,
        output_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        if !self.can_write_to_host_buffer() {
            // Exit early if the host buffer is already occupied
            return Ok(Err(ApiError::HostBufferFull));
        }

        let purse: URef = {
            let bytes = self.bytes_from_mem(purse_ptr, purse_size)?;
            match bytesrepr::deserialize(bytes) {
                Ok(purse) => purse,
                Err(error) => return Ok(Err(error.into())),
            }
        };

        let balance = match self.get_balance(purse)? {
            Some(balance) => balance,
            None => return Ok(Err(ApiError::InvalidPurse)),
        };

        let balance_cl_value = match CLValue::from_t(balance) {
            Ok(cl_value) => cl_value,
            Err(error) => return Ok(Err(error.into())),
        };

        let balance_size = balance_cl_value.inner_bytes().len() as i32;
        if let Err(error) = self.write_host_buffer(balance_cl_value) {
            return Ok(Err(error));
        }

        let balance_size_bytes = balance_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &balance_size_bytes) {
            return Err(Error::Interpreter(error.into()));
        }

        Ok(Ok(()))
    }

    fn get_system_contract(
        &mut self,
        system_contract_index: u32,
        dest_ptr: u32,
        _dest_size: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let contract_hash: ContractHash = match SystemContractType::try_from(system_contract_index)
        {
            Ok(SystemContractType::Mint) => self.get_mint_contract()?,
            Ok(SystemContractType::HandlePayment) => self.get_handle_payment_contract()?,
            Ok(SystemContractType::StandardPayment) => self.get_standard_payment_contract()?,
            Ok(SystemContractType::Auction) => self.get_auction_contract()?,
            Err(error) => return Ok(Err(error)),
        };

        match self.memory().set(dest_ptr, contract_hash.as_ref()) {
            Ok(_) => Ok(Ok(())),
            Err(error) => Err(Error::Interpreter(error.into()).into()),
        }
    }

    /// If host_buffer set, clears the host_buffer and returns value, else None
    pub fn take_host_buffer(&mut self) -> Option<CLValue> {
        self.host_buffer.take()
    }

    /// Checks if a write to host buffer can happen.
    ///
    /// This will check if the host buffer is empty.
    fn can_write_to_host_buffer(&self) -> bool {
        self.host_buffer.is_none()
    }

    /// Overwrites data in host buffer only if it's in empty state
    fn write_host_buffer(&mut self, data: CLValue) -> Result<(), ApiError> {
        match self.host_buffer {
            Some(_) => return Err(ApiError::HostBufferFull),
            None => self.host_buffer = Some(data),
        }
        Ok(())
    }

    fn read_host_buffer(
        &mut self,
        dest_ptr: u32,
        dest_size: usize,
        bytes_written_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let (_cl_type, serialized_value) = match self.take_host_buffer() {
            None => return Ok(Err(ApiError::HostBufferEmpty)),
            Some(cl_value) => cl_value.destructure(),
        };

        if serialized_value.len() > u32::max_value() as usize {
            return Ok(Err(ApiError::OutOfMemory));
        }
        if serialized_value.len() > dest_size {
            return Ok(Err(ApiError::BufferTooSmall));
        }

        // Slice data, so if `dest_size` is larger than host_buffer size, it will take host_buffer
        // as whole.
        let sliced_buf = &serialized_value[..cmp::min(dest_size, serialized_value.len())];
        if let Err(error) = self.memory().set(dest_ptr, sliced_buf) {
            return Err(Error::Interpreter(error.into()));
        }

        let bytes_written = sliced_buf.len() as u32;
        let bytes_written_data = bytes_written.to_le_bytes();

        if let Err(error) = self.memory().set(bytes_written_ptr, &bytes_written_data) {
            return Err(Error::Interpreter(error.into()));
        }

        Ok(Ok(()))
    }

    #[cfg(feature = "test-support")]
    fn print(&mut self, text_ptr: u32, text_size: u32) -> Result<(), Error> {
        let text = self.string_from_mem(text_ptr, text_size)?;
        println!("{}", text);
        Ok(())
    }

    fn get_named_arg_size(
        &mut self,
        name_ptr: u32,
        name_size: usize,
        size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let name_bytes = self.bytes_from_mem(name_ptr, name_size)?;
        let name = String::from_utf8_lossy(&name_bytes);

        let arg_size = match self.context.args().get(&name) {
            Some(arg) if arg.inner_bytes().len() > u32::max_value() as usize => {
                return Ok(Err(ApiError::OutOfMemory));
            }
            Some(arg) => arg.inner_bytes().len() as u32,
            None => return Ok(Err(ApiError::MissingArgument)),
        };

        let arg_size_bytes = arg_size.to_le_bytes(); // Wasm is little-endian

        if let Err(e) = self.memory().set(size_ptr, &arg_size_bytes) {
            return Err(Error::Interpreter(e.into()).into());
        }

        Ok(Ok(()))
    }

    fn get_named_arg(
        &mut self,
        name_ptr: u32,
        name_size: usize,
        output_ptr: u32,
        output_size: usize,
    ) -> Result<Result<(), ApiError>, Error> {
        let name_bytes = self.bytes_from_mem(name_ptr, name_size)?;
        let name = String::from_utf8_lossy(&name_bytes);

        let arg = match self.context.args().get(&name) {
            Some(arg) => arg,
            None => return Ok(Err(ApiError::MissingArgument)),
        };

        if arg.inner_bytes().len() > output_size {
            return Ok(Err(ApiError::OutOfMemory));
        }

        if let Err(e) = self
            .memory()
            .set(output_ptr, &arg.inner_bytes()[..output_size])
        {
            return Err(Error::Interpreter(e.into()).into());
        }

        Ok(Ok(()))
    }

    fn validate_entry_point_access(
        &self,
        package: &ContractPackage,
        access: &EntryPointAccess,
    ) -> Result<(), Error> {
        runtime_context::validate_entry_point_access_with(package, access, |uref| {
            self.context.validate_uref(uref).is_ok()
        })
    }

    /// Remove a user group from access to a contract
    fn remove_contract_user_group(
        &mut self,
        package_key: ContractPackageHash,
        label: Group,
    ) -> Result<Result<(), ApiError>, Error> {
        let mut package: ContractPackage =
            self.context.get_validated_contract_package(package_key)?;

        let group_to_remove = Group::new(label);
        let groups = package.groups_mut();

        // Ensure group exists in groups
        if groups.get(&group_to_remove).is_none() {
            return Ok(Err(contracts::Error::GroupDoesNotExist.into()));
        }

        // Remove group if it is not referenced by at least one entry_point in active versions.
        let versions = package.versions();
        for contract_hash in versions.values() {
            let entry_points = {
                let contract: Contract = self.context.read_gs_typed(&Key::from(*contract_hash))?;
                contract.entry_points().clone().take_entry_points()
            };
            for entry_point in entry_points {
                match entry_point.access() {
                    EntryPointAccess::Public => {
                        continue;
                    }
                    EntryPointAccess::Groups(groups) => {
                        if groups.contains(&group_to_remove) {
                            return Ok(Err(contracts::Error::GroupInUse.into()));
                        }
                    }
                }
            }
        }

        if !package.remove_group(&group_to_remove) {
            return Ok(Err(contracts::Error::GroupInUse.into()));
        }

        // Write updated package to the global state
        self.context.metered_write_gs_unsafe(package_key, package)?;
        Ok(Ok(()))
    }

    #[allow(clippy::too_many_arguments)]
    fn provision_contract_user_group_uref(
        &mut self,
        package_ptr: u32,
        package_size: u32,
        label_ptr: u32,
        label_size: u32,
        output_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let contract_package_hash = self.t_from_mem(package_ptr, package_size)?;
        let label: String = self.t_from_mem(label_ptr, label_size)?;
        let mut contract_package = self
            .context
            .get_validated_contract_package(contract_package_hash)?;
        let groups = contract_package.groups_mut();

        let group_label = Group::new(label);

        // Ensure there are not too many urefs
        let total_urefs: usize = groups.values().map(|urefs| urefs.len()).sum();

        if total_urefs + 1 > contracts::MAX_TOTAL_UREFS {
            return Ok(Err(contracts::Error::MaxTotalURefsExceeded.into()));
        }

        // Ensure given group exists and does not exceed limits
        let group = match groups.get_mut(&group_label) {
            Some(group) if group.len() + 1 > contracts::MAX_GROUPS as usize => {
                // Ensures there are not too many groups to fit in amount of new urefs
                return Ok(Err(contracts::Error::MaxTotalURefsExceeded.into()));
            }
            Some(group) => group,
            None => return Ok(Err(contracts::Error::GroupDoesNotExist.into())),
        };

        // Proceed with creating new URefs
        let new_uref = self.context.new_unit_uref()?;
        if !group.insert(new_uref) {
            return Ok(Err(contracts::Error::URefAlreadyExists.into()));
        }

        // check we can write to the host buffer
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }
        // create CLValue for return value
        let new_uref_value = CLValue::from_t(new_uref)?;
        let value_size = new_uref_value.inner_bytes().len();
        // write return value to buffer
        if let Err(err) = self.write_host_buffer(new_uref_value) {
            return Ok(Err(err));
        }
        // Write return value size to output location
        let output_size_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &output_size_bytes) {
            return Err(Error::Interpreter(error.into()));
        }

        // Write updated package to the global state
        self.context
            .metered_write_gs_unsafe(contract_package_hash, contract_package)?;

        Ok(Ok(()))
    }

    #[allow(clippy::too_many_arguments)]
    fn remove_contract_user_group_urefs(
        &mut self,
        package_ptr: u32,
        package_size: u32,
        label_ptr: u32,
        label_size: u32,
        urefs_ptr: u32,
        urefs_size: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let contract_package_hash: ContractPackageHash =
            self.t_from_mem(package_ptr, package_size)?;
        let label: String = self.t_from_mem(label_ptr, label_size)?;
        let urefs: BTreeSet<URef> = self.t_from_mem(urefs_ptr, urefs_size)?;

        let mut contract_package = self
            .context
            .get_validated_contract_package(contract_package_hash)?;

        let groups = contract_package.groups_mut();
        let group_label = Group::new(label);

        let group = match groups.get_mut(&group_label) {
            Some(group) => group,
            None => return Ok(Err(contracts::Error::GroupDoesNotExist.into())),
        };

        if urefs.is_empty() {
            return Ok(Ok(()));
        }

        for uref in urefs {
            if !group.remove(&uref) {
                return Ok(Err(contracts::Error::UnableToRemoveURef.into()));
            }
        }
        // Write updated package to the global state
        self.context
            .metered_write_gs_unsafe(contract_package_hash, contract_package)?;

        Ok(Ok(()))
    }

    /// Calculate gas cost for a host function
    fn charge_host_function_call<T>(
        &mut self,
        host_function: &HostFunction<T>,
        weights: T,
    ) -> Result<(), Error>
    where
        T: AsRef<[Cost]> + Copy,
    {
        let cost = host_function.calculate_gas_cost(weights);
        self.gas(cost)?;
        Ok(())
    }

    /// Creates a dictionary
    fn new_dictionary(&mut self, output_size_ptr: u32) -> Result<Result<(), ApiError>, Error> {
        // check we can write to the host buffer
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }

        // Create new URef
        let new_uref = self.context.new_unit_uref()?;

        // create CLValue for return value
        let new_uref_value = CLValue::from_t(new_uref)?;
        let value_size = new_uref_value.inner_bytes().len();
        // write return value to buffer
        if let Err(err) = self.write_host_buffer(new_uref_value) {
            return Ok(Err(err));
        }
        // Write return value size to output location
        let output_size_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &output_size_bytes) {
            return Err(Error::Interpreter(error.into()));
        }

        Ok(Ok(()))
    }

    /// Reads the `value` under a `key` in a dictionary
    fn dictionary_get(
        &mut self,
        uref_ptr: u32,
        uref_size: u32,
        dictionary_item_key_bytes_ptr: u32,
        dictionary_item_key_bytes_size: u32,
        output_size_ptr: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        // check we can write to the host buffer
        if let Err(err) = self.check_host_buffer() {
            return Ok(Err(err));
        }

        let uref: URef = self.t_from_mem(uref_ptr, uref_size)?;
        let dictionary_item_key_bytes = self.bytes_from_mem(
            dictionary_item_key_bytes_ptr,
            dictionary_item_key_bytes_size as usize,
        )?;

        let dictionary_item_key = if let Ok(item_key) = String::from_utf8(dictionary_item_key_bytes)
        {
            item_key
        } else {
            return Ok(Err(ApiError::InvalidDictionaryItemKey));
        };

        let cl_value = match self.context.dictionary_get(uref, &dictionary_item_key)? {
            Some(cl_value) => cl_value,
            None => return Ok(Err(ApiError::ValueNotFound)),
        };

        let value_size = cl_value.inner_bytes().len() as u32;
        if let Err(error) = self.write_host_buffer(cl_value) {
            return Ok(Err(error));
        }

        let value_bytes = value_size.to_le_bytes(); // Wasm is little-endian
        if let Err(error) = self.memory().set(output_size_ptr, &value_bytes) {
            return Err(Error::Interpreter(error.into()).into());
        }

        Ok(Ok(()))
    }

    /// Writes a `key`, `value` pair in a dictionary
    fn dictionary_put(
        &mut self,
        uref_ptr: u32,
        uref_size: u32,
        key_ptr: u32,
        key_size: u32,
        value_ptr: u32,
        value_size: u32,
    ) -> Result<Result<(), ApiError>, Error> {
        let uref: URef = self.t_from_mem(uref_ptr, uref_size)?;
        let dictionary_item_key_bytes = self.bytes_from_mem(key_ptr, key_size as usize)?;
        if dictionary_item_key_bytes.len() > DICTIONARY_ITEM_KEY_MAX_LENGTH {
            return Ok(Err(ApiError::DictionaryItemKeyExceedsLength));
        }
        let dictionary_item_key = if let Ok(item_key) = String::from_utf8(dictionary_item_key_bytes)
        {
            item_key
        } else {
            return Ok(Err(ApiError::InvalidDictionaryItemKey));
        };
        let cl_value = self.cl_value_from_mem(value_ptr, value_size)?;
        self.context
            .dictionary_put(uref, &dictionary_item_key, cl_value)?;
        Ok(Ok(()))
    }

    /// Reverts contract execution with a status specified.
    pub fn casper_revert(&mut self, status: u32) -> Result<(), Error> {
        // Err(Error::Revert(api_error:))
        let host_function_costs = self.config.wasm_config().take_host_function_costs();
        self.charge_host_function_call(&host_function_costs.revert, [status])?;
        Err(Error::Revert(status.into()))
    }

    pub fn memory(&self) -> &MemoryRef {
        match &self.instance {
            Instance::Interpreted(_module, memory_ref) => memory_ref,
        }
    }

    // pub fn invoke_wasm_function(&self, func: FunctionIndex, args: &[RuntimeValue]) ->
    // Result<Option<RuntimeValue>, Error> {

    // let mut scoped_instrumenter = ScopedInstrumenter::new(func);

    // // let host_function_costs = self.config.wasm_config().take_host_function_costs();
    // match func {
    //     FunctionIndex::RevertFuncIndex => {

    //     }
    // }

    // match func {
    //     FunctionIndex::ReadFuncIndex => {
    //         // args(0) = pointer to key in Wasm memory
    //         // args(1) = size of key in Wasm memory
    //         // args(2) = pointer to output size (output param)
    //         let (key_ptr, key_size, output_size_ptr) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.read_value,
    //             [key_ptr, key_size, output_size_ptr],
    //         )?;
    //         let ret = self.read(key_ptr, key_size, output_size_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::LoadNamedKeysFuncIndex => {
    //         // args(0) = pointer to amount of keys (output)
    //         // args(1) = pointer to amount of serialized bytes (output)
    //         let (total_keys_ptr, result_size_ptr) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.load_named_keys,
    //             [total_keys_ptr, result_size_ptr],
    //         )?;
    //         let ret = self.load_named_keys(
    //             total_keys_ptr,
    //             result_size_ptr,
    //             &mut scoped_instrumenter,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::WriteFuncIndex => {
    //         // args(0) = pointer to key in Wasm memory
    //         // args(1) = size of key
    //         // args(2) = pointer to value
    //         // args(3) = size of value
    //         let (key_ptr, key_size, value_ptr, value_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.write,
    //             [key_ptr, key_size, value_ptr, value_size],
    //         )?;
    //         scoped_instrumenter.add_property("value_size", value_size);
    //         self.write(key_ptr, key_size, value_ptr, value_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::AddFuncIndex => {
    //         // args(0) = pointer to key in Wasm memory
    //         // args(1) = size of key
    //         // args(2) = pointer to value
    //         // args(3) = size of value
    //         let (key_ptr, key_size, value_ptr, value_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.add,
    //             [key_ptr, key_size, value_ptr, value_size],
    //         )?;
    //         self.add(key_ptr, key_size, value_ptr, value_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::NewFuncIndex => {
    //         // args(0) = pointer to uref destination in Wasm memory
    //         // args(1) = pointer to initial value
    //         // args(2) = size of initial value
    //         let (uref_ptr, value_ptr, value_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.new_uref,
    //             [uref_ptr, value_ptr, value_size],
    //         )?;
    //         scoped_instrumenter.add_property("value_size", value_size);
    //         self.new_uref(uref_ptr, value_ptr, value_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::RetFuncIndex => {
    //         // args(0) = pointer to value
    //         // args(1) = size of value
    //         let (value_ptr, value_size) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.ret, [value_ptr, value_size])?;
    //         scoped_instrumenter.add_property("value_size", value_size);
    //         Err(self.ret(value_ptr, value_size as usize, &mut scoped_instrumenter))
    //     }

    //     FunctionIndex::GetKeyFuncIndex => {
    //         // args(0) = pointer to key name in Wasm memory
    //         // args(1) = size of key name
    //         // args(2) = pointer to output buffer for serialized key
    //         // args(3) = size of output buffer
    //         // args(4) = pointer to bytes written
    //         let (name_ptr, name_size, output_ptr, output_size, bytes_written) =
    //             Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.get_key,
    //             [name_ptr, name_size, output_ptr, output_size, bytes_written],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size);
    //         let ret = self.load_key(
    //             name_ptr,
    //             name_size,
    //             output_ptr,
    //             output_size as usize,
    //             bytes_written,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::HasKeyFuncIndex => {
    //         // args(0) = pointer to key name in Wasm memory
    //         // args(1) = size of key name
    //         let (name_ptr, name_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.has_key,
    //             [name_ptr, name_size],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size);
    //         let result = self.has_key(name_ptr, name_size)?;
    //         Ok(Some(RuntimeValue::I32(result)))
    //     }

    //     FunctionIndex::PutKeyFuncIndex => {
    //         // args(0) = pointer to key name in Wasm memory
    //         // args(1) = size of key name
    //         // args(2) = pointer to key in Wasm memory
    //         // args(3) = size of key
    //         let (name_ptr, name_size, key_ptr, key_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.put_key,
    //             [name_ptr, name_size, key_ptr, key_size],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size);
    //         self.put_key(name_ptr, name_size, key_ptr, key_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::RemoveKeyFuncIndex => {
    //         // args(0) = pointer to key name in Wasm memory
    //         // args(1) = size of key name
    //         let (name_ptr, name_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.remove_key,
    //             [name_ptr, name_size],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size);
    //         self.remove_key(name_ptr, name_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::GetCallerIndex => {
    //         // args(0) = pointer where a size of serialized bytes will be stored
    //         let (output_size,) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.get_caller, [output_size])?;
    //         let ret = self.get_caller(output_size)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::GetBlocktimeIndex => {
    //         // args(0) = pointer to Wasm memory where to write.
    //         let (dest_ptr,) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.get_blocktime, [dest_ptr])?;
    //         self.get_blocktime(dest_ptr)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::GasFuncIndex => {
    //         let (gas_arg,): (u32,) = Args::parse(args)?;
    //         // Gas is special cased internal host function and for accounting purposes it isn't
    //         // represented in protocol data.
    //         self.gas(Gas::new(gas_arg.into()))?;
    //         Ok(None)
    //     }

    //     FunctionIndex::IsValidURefFnIndex => {
    //         // args(0) = pointer to value to validate
    //         // args(1) = size of value
    //         let (uref_ptr, uref_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.is_valid_uref,
    //             [uref_ptr, uref_size],
    //         )?;
    //         Ok(Some(RuntimeValue::I32(i32::from(
    //             self.is_valid_uref(uref_ptr, uref_size)?,
    //         ))))
    //     }

    //     FunctionIndex::RevertFuncIndex => {
    //         // args(0) = status u32
    //         let (status,) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.revert, [status])?;
    //         Err(self.revert(status))
    //     }

    //     FunctionIndex::AddAssociatedKeyFuncIndex => {
    //         // args(0) = pointer to array of bytes of an account hash
    //         // args(1) = size of an account hash
    //         // args(2) = weight of the key
    //         let (account_hash_ptr, account_hash_size, weight_value) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.add_associated_key,
    //             [account_hash_ptr, account_hash_size, weight_value as Cost],
    //         )?;
    //         let value = self.add_associated_key(
    //             account_hash_ptr,
    //             account_hash_size as usize,
    //             weight_value,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(value)))
    //     }

    //     FunctionIndex::RemoveAssociatedKeyFuncIndex => {
    //         // args(0) = pointer to array of bytes of an account hash
    //         // args(1) = size of an account hash
    //         let (account_hash_ptr, account_hash_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.remove_associated_key,
    //             [account_hash_ptr, account_hash_size],
    //         )?;
    //         let value =
    //             self.remove_associated_key(account_hash_ptr, account_hash_size as usize)?;
    //         Ok(Some(RuntimeValue::I32(value)))
    //     }

    //     FunctionIndex::UpdateAssociatedKeyFuncIndex => {
    //         // args(0) = pointer to array of bytes of an account hash
    //         // args(1) = size of an account hash
    //         // args(2) = weight of the key
    //         let (account_hash_ptr, account_hash_size, weight_value) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.update_associated_key,
    //             [account_hash_ptr, account_hash_size, weight_value as Cost],
    //         )?;
    //         let value = self.update_associated_key(
    //             account_hash_ptr,
    //             account_hash_size as usize,
    //             weight_value,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(value)))
    //     }

    //     FunctionIndex::SetActionThresholdFuncIndex => {
    //         // args(0) = action type
    //         // args(1) = new threshold
    //         let (action_type_value, threshold_value) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.set_action_threshold,
    //             [action_type_value, threshold_value as Cost],
    //         )?;
    //         let value = self.set_action_threshold(action_type_value, threshold_value)?;
    //         Ok(Some(RuntimeValue::I32(value)))
    //     }

    //     FunctionIndex::CreatePurseIndex => {
    //         // args(0) = pointer to array for return value
    //         // args(1) = length of array for return value
    //         let (dest_ptr, dest_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.create_purse,
    //             [dest_ptr, dest_size],
    //         )?;
    //         let purse = self.create_purse()?;
    //         let purse_bytes = purse.into_bytes().map_err(Error::BytesRepr)?;
    //         assert_eq!(dest_size, purse_bytes.len() as u32);
    //         self.instance.memory()
    //             .set(dest_ptr, &purse_bytes)
    //             .map_err(|e| Error::Interpreter(e.into()))?;
    //         Ok(Some(RuntimeValue::I32(0)))
    //     }

    //     FunctionIndex::TransferToAccountIndex => {
    //         // args(0) = pointer to array of bytes of an account hash
    //         // args(1) = length of array of bytes of an account hash
    //         // args(2) = pointer to array of bytes of an amount
    //         // args(3) = length of array of bytes of an amount
    //         // args(4) = pointer to array of bytes of an id
    //         // args(5) = length of array of bytes of an id
    //         // args(6) = pointer to a value where new value will be set
    //         let (key_ptr, key_size, amount_ptr, amount_size, id_ptr, id_size, result_ptr) =
    //             Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.transfer_to_account,
    //             [
    //                 key_ptr,
    //                 key_size,
    //                 amount_ptr,
    //                 amount_size,
    //                 id_ptr,
    //                 id_size,
    //                 result_ptr,
    //             ],
    //         )?;
    //         let account_hash: AccountHash = {
    //             let bytes = self.bytes_from_mem(key_ptr, key_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let amount: U512 = {
    //             let bytes = self.bytes_from_mem(amount_ptr, amount_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let id: Option<u64> = {
    //             let bytes = self.bytes_from_mem(id_ptr, id_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };

    //         let ret = match self.transfer_to_account(account_hash, amount, id)? {
    //             Ok(transferred_to) => {
    //                 let result_value: u32 = transferred_to as u32;
    //                 let result_value_bytes = result_value.to_le_bytes();
    //                 self.memory()
    //                     .set(result_ptr, &result_value_bytes)
    //                     .map_err(|error| Error::Interpreter(error.into()))?;
    //                 Ok(())
    //             }
    //             Err(api_error) => Err(api_error),
    //         };
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::TransferFromPurseToAccountIndex => {
    //         // args(0) = pointer to array of bytes in Wasm memory of a source purse
    //         // args(1) = length of array of bytes in Wasm memory of a source purse
    //         // args(2) = pointer to array of bytes in Wasm memory of an account hash
    //         // args(3) = length of array of bytes in Wasm memory of an account hash
    //         // args(4) = pointer to array of bytes in Wasm memory of an amount
    //         // args(5) = length of array of bytes in Wasm memory of an amount
    //         // args(6) = pointer to array of bytes in Wasm memory of an id
    //         // args(7) = length of array of bytes in Wasm memory of an id
    //         // args(8) = pointer to a value where value of `TransferredTo` enum will be set
    //         let (
    //             source_ptr,
    //             source_size,
    //             key_ptr,
    //             key_size,
    //             amount_ptr,
    //             amount_size,
    //             id_ptr,
    //             id_size,
    //             result_ptr,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.transfer_from_purse_to_account,
    //             [
    //                 source_ptr,
    //                 source_size,
    //                 key_ptr,
    //                 key_size,
    //                 amount_ptr,
    //                 amount_size,
    //                 id_ptr,
    //                 id_size,
    //                 result_ptr,
    //             ],
    //         )?;
    //         let source_purse = {
    //             let bytes = self.bytes_from_mem(source_ptr, source_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let account_hash: AccountHash = {
    //             let bytes = self.bytes_from_mem(key_ptr, key_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let amount: U512 = {
    //             let bytes = self.bytes_from_mem(amount_ptr, amount_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let id: Option<u64> = {
    //             let bytes = self.bytes_from_mem(id_ptr, id_size as usize)?;
    //             bytesrepr::deserialize(bytes).map_err(Error::BytesRepr)?
    //         };
    //         let ret = match self.transfer_from_purse_to_account(
    //             source_purse,
    //             account_hash,
    //             amount,
    //             id,
    //         )? {
    //             Ok(transferred_to) => {
    //                 let result_value: u32 = transferred_to as u32;
    //                 let result_value_bytes = result_value.to_le_bytes();
    //                 self.memory()
    //                     .set(result_ptr, &result_value_bytes)
    //                     .map_err(|error| Error::Interpreter(error.into()))?;
    //                 Ok(())
    //             }
    //             Err(api_error) => Err(api_error),
    //         };
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::TransferFromPurseToPurseIndex => {
    //         // args(0) = pointer to array of bytes in Wasm memory of a source purse
    //         // args(1) = length of array of bytes in Wasm memory of a source purse
    //         // args(2) = pointer to array of bytes in Wasm memory of a target purse
    //         // args(3) = length of array of bytes in Wasm memory of a target purse
    //         // args(4) = pointer to array of bytes in Wasm memory of an amount
    //         // args(5) = length of array of bytes in Wasm memory of an amount
    //         // args(6) = pointer to array of bytes in Wasm memory of an id
    //         // args(7) = length of array of bytes in Wasm memory of an id
    //         let (
    //             source_ptr,
    //             source_size,
    //             target_ptr,
    //             target_size,
    //             amount_ptr,
    //             amount_size,
    //             id_ptr,
    //             id_size,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.transfer_from_purse_to_purse,
    //             [
    //                 source_ptr,
    //                 source_size,
    //                 target_ptr,
    //                 target_size,
    //                 amount_ptr,
    //                 amount_size,
    //                 id_ptr,
    //                 id_size,
    //             ],
    //         )?;
    //         let ret = self.transfer_from_purse_to_purse(
    //             source_ptr,
    //             source_size,
    //             target_ptr,
    //             target_size,
    //             amount_ptr,
    //             amount_size,
    //             id_ptr,
    //             id_size,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::GetBalanceIndex => {
    //         // args(0) = pointer to purse input
    //         // args(1) = length of purse
    //         // args(2) = pointer to output size (output)
    //         let (ptr, ptr_size, output_size_ptr) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.get_balance,
    //             [ptr, ptr_size, output_size_ptr],
    //         )?;
    //         let ret = self.get_balance_host_buffer(ptr, ptr_size as usize, output_size_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::GetPhaseIndex => {
    //         // args(0) = pointer to Wasm memory where to write.
    //         let (dest_ptr,) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.get_phase, [dest_ptr])?;
    //         self.get_phase(dest_ptr)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::GetSystemContractIndex => {
    //         // args(0) = system contract index
    //         // args(1) = dest pointer for storing serialized result
    //         // args(2) = dest pointer size
    //         let (system_contract_index, dest_ptr, dest_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.get_system_contract,
    //             [system_contract_index, dest_ptr, dest_size],
    //         )?;
    //         let ret = self.get_system_contract(system_contract_index, dest_ptr, dest_size)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::GetMainPurseIndex => {
    //         // args(0) = pointer to Wasm memory where to write.
    //         let (dest_ptr,) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.get_main_purse, [dest_ptr])?;
    //         self.get_main_purse(dest_ptr)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::ReadHostBufferIndex => {
    //         // args(0) = pointer to Wasm memory where to write size.
    //         let (dest_ptr, dest_size, bytes_written_ptr) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.read_host_buffer,
    //             [dest_ptr, dest_size, bytes_written_ptr],
    //         )?;
    //         scoped_instrumenter.add_property("dest_size", dest_size);
    //         let ret = self.read_host_buffer(dest_ptr, dest_size as usize, bytes_written_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::CreateContractPackageAtHash => {
    //         // args(0) = pointer to wasm memory where to write 32-byte Hash address
    //         // args(1) = pointer to wasm memory where to write 32-byte access key address
    //         // args(2) = boolean flag to determine if the contract can be versioned
    //         let (hash_dest_ptr, access_dest_ptr, is_locked) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.create_contract_package_at_hash,
    //             [hash_dest_ptr, access_dest_ptr],
    //         )?;
    //         let package_status = ContractPackageStatus::new(is_locked);
    //         let (hash_addr, access_addr) =
    //             self.create_contract_package_at_hash(package_status)?;

    //         self.function_address(hash_addr, hash_dest_ptr)?;
    //         self.function_address(access_addr, access_dest_ptr)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::CreateContractUserGroup => {
    //         // args(0) = pointer to package key in wasm memory
    //         // args(1) = size of package key in wasm memory
    //         // args(2) = pointer to group label in wasm memory
    //         // args(3) = size of group label in wasm memory
    //         // args(4) = number of new urefs to generate for the group
    //         // args(5) = pointer to existing_urefs in wasm memory
    //         // args(6) = size of existing_urefs in wasm memory
    //         // args(7) = pointer to location to write size of output (written to host buffer)
    //         let (
    //             package_key_ptr,
    //             package_key_size,
    //             label_ptr,
    //             label_size,
    //             num_new_urefs,
    //             existing_urefs_ptr,
    //             existing_urefs_size,
    //             output_size_ptr,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.create_contract_user_group,
    //             [
    //                 package_key_ptr,
    //                 package_key_size,
    //                 label_ptr,
    //                 label_size,
    //                 num_new_urefs,
    //                 existing_urefs_ptr,
    //                 existing_urefs_size,
    //                 output_size_ptr,
    //             ],
    //         )?;
    //         scoped_instrumenter
    //             .add_property("existing_urefs_size", existing_urefs_size.to_string());
    //         scoped_instrumenter.add_property("label_size", label_size.to_string());

    //         let contract_package_hash: ContractPackageHash =
    //             self.t_from_mem(package_key_ptr, package_key_size)?;
    //         let label: String = self.t_from_mem(label_ptr, label_size)?;
    //         let existing_urefs: BTreeSet<URef> =
    //             self.t_from_mem(existing_urefs_ptr, existing_urefs_size)?;

    //         let ret = self.create_contract_user_group(
    //             contract_package_hash,
    //             label,
    //             num_new_urefs,
    //             existing_urefs,
    //             output_size_ptr,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::AddContractVersion => {
    //         // args(0) = pointer to package key in wasm memory
    //         // args(1) = size of package key in wasm memory
    //         // args(2) = pointer to entrypoints in wasm memory
    //         // args(3) = size of entrypoints in wasm memory
    //         // args(4) = pointer to named keys in wasm memory
    //         // args(5) = size of named keys in wasm memory
    //         // args(6) = pointer to output buffer for serialized key
    //         // args(7) = size of output buffer
    //         // args(8) = pointer to bytes written
    //         let (
    //             contract_package_hash_ptr,
    //             contract_package_hash_size,
    //             version_ptr,
    //             entry_points_ptr,
    //             entry_points_size,
    //             named_keys_ptr,
    //             named_keys_size,
    //             output_ptr,
    //             output_size,
    //             bytes_written_ptr,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.add_contract_version,
    //             [
    //                 contract_package_hash_ptr,
    //                 contract_package_hash_size,
    //                 version_ptr,
    //                 entry_points_ptr,
    //                 entry_points_size,
    //                 named_keys_ptr,
    //                 named_keys_size,
    //                 output_ptr,
    //                 output_size,
    //                 bytes_written_ptr,
    //             ],
    //         )?;
    //         scoped_instrumenter
    //             .add_property("entry_points_size", entry_points_size.to_string());
    //         scoped_instrumenter.add_property("named_keys_size", named_keys_size.to_string());

    //         let contract_package_hash: ContractPackageHash =
    //             self.t_from_mem(contract_package_hash_ptr, contract_package_hash_size)?;
    //         let entry_points: EntryPoints =
    //             self.t_from_mem(entry_points_ptr, entry_points_size)?;
    //         let named_keys: NamedKeys = self.t_from_mem(named_keys_ptr, named_keys_size)?;
    //         let ret = self.add_contract_version(
    //             contract_package_hash,
    //             entry_points,
    //             named_keys,
    //             output_ptr,
    //             output_size as usize,
    //             bytes_written_ptr,
    //             version_ptr,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::DisableContractVersion => {
    //         // args(0) = pointer to package hash in wasm memory
    //         // args(1) = size of package hash in wasm memory
    //         // args(2) = pointer to contract hash in wasm memory
    //         // args(3) = size of contract hash in wasm memory
    //         let (package_key_ptr, package_key_size, contract_hash_ptr, contract_hash_size) =
    //             Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.disable_contract_version,
    //             [
    //                 package_key_ptr,
    //                 package_key_size,
    //                 contract_hash_ptr,
    //                 contract_hash_size,
    //             ],
    //         )?;
    //         let contract_package_hash = self.t_from_mem(package_key_ptr, package_key_size)?;
    //         let contract_hash = self.t_from_mem(contract_hash_ptr, contract_hash_size)?;

    //         let result = self.disable_contract_version(contract_package_hash, contract_hash)?;

    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(result))))
    //     }

    //     FunctionIndex::CallContractFuncIndex => {
    //         // args(0) = pointer to contract hash where contract is at in global state
    //         // args(1) = size of contract hash
    //         // args(2) = pointer to entry point
    //         // args(3) = size of entry point
    //         // args(4) = pointer to function arguments in Wasm memory
    //         // args(5) = size of arguments
    //         // args(6) = pointer to result size (output)
    //         let (
    //             contract_hash_ptr,
    //             contract_hash_size,
    //             entry_point_name_ptr,
    //             entry_point_name_size,
    //             args_ptr,
    //             args_size,
    //             result_size_ptr,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.call_contract,
    //             [
    //                 contract_hash_ptr,
    //                 contract_hash_size,
    //                 entry_point_name_ptr,
    //                 entry_point_name_size,
    //                 args_ptr,
    //                 args_size,
    //                 result_size_ptr,
    //             ],
    //         )?;
    //         scoped_instrumenter
    //             .add_property("entry_point_name_size", entry_point_name_size.to_string());
    //         scoped_instrumenter.add_property("args_size", args_size.to_string());

    //         let contract_hash: ContractHash =
    //             self.t_from_mem(contract_hash_ptr, contract_hash_size)?;
    //         let entry_point_name: String =
    //             self.t_from_mem(entry_point_name_ptr, entry_point_name_size)?;
    //         let args_bytes: Vec<u8> = {
    //             let args_size: u32 = args_size;
    //             self.bytes_from_mem(args_ptr, args_size as usize)?
    //         };

    //         let ret = self.call_contract_host_buffer(
    //             contract_hash,
    //             &entry_point_name,
    //             args_bytes,
    //             result_size_ptr,
    //             &mut scoped_instrumenter,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::CallVersionedContract => {
    //         // args(0) = pointer to contract_package_hash where contract is at in global state
    //         // args(1) = size of contract_package_hash
    //         // args(2) = pointer to contract version in wasm memory
    //         // args(3) = size of contract version in wasm memory
    //         // args(4) = pointer to method name in wasm memory
    //         // args(5) = size of method name in wasm memory
    //         // args(6) = pointer to function arguments in Wasm memory
    //         // args(7) = size of arguments
    //         // args(8) = pointer to result size (output)
    //         let (
    //             contract_package_hash_ptr,
    //             contract_package_hash_size,
    //             contract_version_ptr,
    //             contract_package_size,
    //             entry_point_name_ptr,
    //             entry_point_name_size,
    //             args_ptr,
    //             args_size,
    //             result_size_ptr,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.call_versioned_contract,
    //             [
    //                 contract_package_hash_ptr,
    //                 contract_package_hash_size,
    //                 contract_version_ptr,
    //                 contract_package_size,
    //                 entry_point_name_ptr,
    //                 entry_point_name_size,
    //                 args_ptr,
    //                 args_size,
    //                 result_size_ptr,
    //             ],
    //         )?;
    //         scoped_instrumenter
    //             .add_property("entry_point_name_size", entry_point_name_size.to_string());
    //         scoped_instrumenter.add_property("args_size", args_size.to_string());

    //         let contract_package_hash: ContractPackageHash =
    //             self.t_from_mem(contract_package_hash_ptr, contract_package_hash_size)?;
    //         let contract_version: Option<ContractVersion> =
    //             self.t_from_mem(contract_version_ptr, contract_package_size)?;
    //         let entry_point_name: String =
    //             self.t_from_mem(entry_point_name_ptr, entry_point_name_size)?;
    //         let args_bytes: Vec<u8> = {
    //             let args_size: u32 = args_size;
    //             self.bytes_from_mem(args_ptr, args_size as usize)?
    //         };

    //         let ret = self.call_versioned_contract_host_buffer(
    //             contract_package_hash,
    //             contract_version,
    //             entry_point_name,
    //             args_bytes,
    //             result_size_ptr,
    //             &mut scoped_instrumenter,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     #[cfg(feature = "test-support")]
    //     FunctionIndex::PrintIndex => {
    //         let (text_ptr, text_size) = Args::parse(args)?;
    //         self.charge_host_function_call(&host_function_costs.print, [text_ptr, text_size])?;
    //         scoped_instrumenter.add_property("text_size", text_size);
    //         self.print(text_ptr, text_size)?;
    //         Ok(None)
    //     }

    //     FunctionIndex::GetRuntimeArgsizeIndex => {
    //         // args(0) = pointer to name of host runtime arg to load
    //         // args(1) = size of name of the host runtime arg
    //         // args(2) = pointer to a argument size (output)
    //         let (name_ptr, name_size, size_ptr) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.get_named_arg_size,
    //             [name_ptr, name_size, size_ptr],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size.to_string());
    //         let ret = self.get_named_arg_size(name_ptr, name_size as usize, size_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::GetRuntimeArgIndex => {
    //         // args(0) = pointer to serialized argument name
    //         // args(1) = size of serialized argument name
    //         // args(2) = pointer to output pointer where host will write argument bytes
    //         // args(3) = size of available data under output pointer
    //         let (name_ptr, name_size, dest_ptr, dest_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.get_named_arg,
    //             [name_ptr, name_size, dest_ptr, dest_size],
    //         )?;
    //         scoped_instrumenter.add_property("name_size", name_size.to_string());
    //         scoped_instrumenter.add_property("dest_size", dest_size.to_string());
    //         let ret =
    //             self.get_named_arg(name_ptr, name_size as usize, dest_ptr, dest_size as usize)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::RemoveContractUserGroupIndex => {
    //         // args(0) = pointer to package key in wasm memory
    //         // args(1) = size of package key in wasm memory
    //         // args(2) = pointer to serialized group label
    //         // args(3) = size of serialized group label
    //         let (package_key_ptr, package_key_size, label_ptr, label_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.remove_contract_user_group,
    //             [package_key_ptr, package_key_size, label_ptr, label_size],
    //         )?;
    //         scoped_instrumenter.add_property("label_size", label_size.to_string());
    //         let package_key = self.t_from_mem(package_key_ptr, package_key_size)?;
    //         let label: Group = self.t_from_mem(label_ptr, label_size)?;

    //         let ret = self.remove_contract_user_group(package_key, label)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::ExtendContractUserGroupURefsIndex => {
    //         // args(0) = pointer to package key in wasm memory
    //         // args(1) = size of package key in wasm memory
    //         // args(2) = pointer to label name
    //         // args(3) = label size bytes
    //         // args(4) = output of size value of host bytes data
    //         let (package_ptr, package_size, label_ptr, label_size, value_size_ptr) =
    //             Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.provision_contract_user_group_uref,
    //             [
    //                 package_ptr,
    //                 package_size,
    //                 label_ptr,
    //                 label_size,
    //                 value_size_ptr,
    //             ],
    //         )?;
    //         scoped_instrumenter.add_property("label_size", label_size.to_string());
    //         let ret = self.provision_contract_user_group_uref(
    //             package_ptr,
    //             package_size,
    //             label_ptr,
    //             label_size,
    //             value_size_ptr,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::RemoveContractUserGroupURefsIndex => {
    //         // args(0) = pointer to package key in wasm memory
    //         // args(1) = size of package key in wasm memory
    //         // args(2) = pointer to label name
    //         // args(3) = label size bytes
    //         // args(4) = pointer to urefs
    //         // args(5) = size of urefs pointer
    //         let (package_ptr, package_size, label_ptr, label_size, urefs_ptr, urefs_size) =
    //             Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.remove_contract_user_group_urefs,
    //             [
    //                 package_ptr,
    //                 package_size,
    //                 label_ptr,
    //                 label_size,
    //                 urefs_ptr,
    //                 urefs_size,
    //             ],
    //         )?;
    //         scoped_instrumenter.add_property("label_size", label_size.to_string());
    //         scoped_instrumenter.add_property("urefs_size", urefs_size.to_string());
    //         let ret = self.remove_contract_user_group_urefs(
    //             package_ptr,
    //             package_size,
    //             label_ptr,
    //             label_size,
    //             urefs_ptr,
    //             urefs_size,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }

    //     FunctionIndex::Blake2b => {
    //         let (in_ptr, in_size, out_ptr, out_size) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.blake2b,
    //             [in_ptr, in_size, out_ptr, out_size],
    //         )?;
    //         scoped_instrumenter.add_property("in_size", in_size.to_string());
    //         scoped_instrumenter.add_property("out_size", out_size.to_string());
    //         let input: Vec<u8> = self.bytes_from_mem(in_ptr, in_size as usize)?;
    //         let digest = account::blake2b(&input);
    //         if digest.len() != out_size as usize {
    //             let err_value = u32::from(api_error::ApiError::BufferTooSmall) as i32;
    //             return Ok(Some(RuntimeValue::I32(err_value)));
    //         }
    //         self.memory()
    //             .set(out_ptr, &digest)
    //             .map_err(|error| Error::Interpreter(error.into()))?;
    //         Ok(Some(RuntimeValue::I32(0)))
    //     }

    //     FunctionIndex::RecordTransfer => {
    //         // RecordTransfer is a special cased internal host function only callable by the
    //         // mint contract and for accounting purposes it isn't represented in protocol data.
    //         let (
    //             maybe_to_ptr,
    //             maybe_to_size,
    //             source_ptr,
    //             source_size,
    //             target_ptr,
    //             target_size,
    //             amount_ptr,
    //             amount_size,
    //             id_ptr,
    //             id_size,
    //         ): (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) = Args::parse(args)?;
    //         scoped_instrumenter.add_property("maybe_to_size", maybe_to_size.to_string());
    //         scoped_instrumenter.add_property("source_size", source_size.to_string());
    //         scoped_instrumenter.add_property("target_size", target_size.to_string());
    //         scoped_instrumenter.add_property("amount_size", amount_size.to_string());
    //         scoped_instrumenter.add_property("id_size", id_size.to_string());
    //         let maybe_to: Option<AccountHash> = self.t_from_mem(maybe_to_ptr, maybe_to_size)?;
    //         let source: URef = self.t_from_mem(source_ptr, source_size)?;
    //         let target: URef = self.t_from_mem(target_ptr, target_size)?;
    //         let amount: U512 = self.t_from_mem(amount_ptr, amount_size)?;
    //         let id: Option<u64> = self.t_from_mem(id_ptr, id_size)?;
    //         self.record_transfer(maybe_to, source, target, amount, id)?;
    //         Ok(Some(RuntimeValue::I32(0)))
    //     }

    //     FunctionIndex::RecordEraInfo => {
    //         // RecordEraInfo is a special cased internal host function only callable by the
    //         // auction contract and for accounting purposes it isn't represented in protocol
    //         // data.
    //         let (era_id_ptr, era_id_size, era_info_ptr, era_info_size): (u32, u32, u32, u32) =
    //             Args::parse(args)?;
    //         scoped_instrumenter.add_property("era_id_size", era_id_size.to_string());
    //         scoped_instrumenter.add_property("era_info_size", era_info_size.to_string());
    //         let era_id: EraId = self.t_from_mem(era_id_ptr, era_id_size)?;
    //         let era_info: EraInfo = self.t_from_mem(era_info_ptr, era_info_size)?;
    //         self.record_era_info(era_id, era_info)?;
    //         Ok(Some(RuntimeValue::I32(0)))
    //     }
    //     FunctionIndex::NewDictionaryFuncIndex => {
    //         // args(0) = pointer to output size (output param)
    //         let (output_size_ptr,): (u32,) = Args::parse(args)?;

    //         self.charge_host_function_call(
    //             &DEFAULT_HOST_FUNCTION_NEW_DICTIONARY,
    //             [output_size_ptr],
    //         )?;
    //         let ret = self.new_dictionary(output_size_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }
    //     FunctionIndex::DictionaryGetFuncIndex => {
    //         // args(0) = pointer to uref in Wasm memory
    //         // args(1) = size of uref in Wasm memory
    //         // args(2) = pointer to key bytes pointer in Wasm memory
    //         // args(3) = pointer to key bytes size in Wasm memory
    //         // args(4) = pointer to output size (output param)
    //         let (uref_ptr, uref_size, key_bytes_ptr, key_bytes_size, output_size_ptr): (
    //             _,
    //             u32,
    //             _,
    //             u32,
    //             _,
    //         ) = Args::parse(args)?;
    //         self.charge_host_function_call(
    //             &host_function_costs.dictionary_get,
    //             [key_bytes_ptr, key_bytes_size, output_size_ptr],
    //         )?;
    //         scoped_instrumenter.add_property("key_bytes_size", key_bytes_size);
    //         let ret = self.dictionary_get(
    //             uref_ptr,
    //             uref_size,
    //             key_bytes_ptr,
    //             key_bytes_size,
    //             output_size_ptr,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }
    //     FunctionIndex::DictionaryPutFuncIndex => {
    //         // args(0) = pointer to uref in Wasm memory
    //         // args(1) = size of uref in Wasm memory
    //         // args(2) = pointer to key bytes pointer in Wasm memory
    //         // args(3) = pointer to key bytes size in Wasm memory
    //         // args(4) = pointer to value bytes pointer in Wasm memory
    //         // args(5) = pointer to value bytes size in Wasm memory
    //         let (uref_ptr, uref_size, key_bytes_ptr, key_bytes_size, value_ptr, value_ptr_size):
    // (_, u32, _, u32, _, u32) = Args::parse(args)?;         self.charge_host_function_call(
    //             &host_function_costs.dictionary_put,
    //             [key_bytes_ptr, key_bytes_size, value_ptr, value_ptr_size],
    //         )?;
    //         scoped_instrumenter.add_property("key_bytes_size", key_bytes_size);
    //         scoped_instrumenter.add_property("value_size", value_ptr_size);
    //         let ret = self.dictionary_put(
    //             uref_ptr,
    //             uref_size,
    //             key_bytes_ptr,
    //             key_bytes_size,
    //             value_ptr,
    //             value_ptr_size,
    //         )?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }
    //     FunctionIndex::LoadCallStack => {
    //         // args(0) (Output) Pointer to number of elements in the call stack.
    //         // args(1) (Output) Pointer to size in bytes of the serialized call stack.
    //         let (call_stack_len_ptr, result_size_ptr) = Args::parse(args)?;
    //         // TODO: add cost table entry once we can upgrade safely
    //         self.charge_host_function_call(
    //             &HostFunction::fixed(10_000),
    //             [call_stack_len_ptr, result_size_ptr],
    //         )?;
    //         let ret = self.load_call_stack(call_stack_len_ptr, result_size_ptr)?;
    //         Ok(Some(RuntimeValue::I32(api_error::i32_from(ret))))
    //     }
    // }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::{
        array::uniform32,
        collection::{btree_map, vec},
        option,
        prelude::*,
        result,
    };

    use casper_types::{gens::*, AccessRights, CLType, CLValue, Key, PublicKey, SecretKey, URef};

    use super::extract_urefs;

    fn cl_value_with_urefs_arb() -> impl Strategy<Value = (CLValue, Vec<URef>)> {
        // If compiler brings you here it most probably means you've added a variant to `CLType`
        // enum but forgot to add generator for it.
        let stub: Option<CLType> = None;
        if let Some(cl_type) = stub {
            match cl_type {
                CLType::Bool
                | CLType::I32
                | CLType::I64
                | CLType::U8
                | CLType::U32
                | CLType::U64
                | CLType::U128
                | CLType::U256
                | CLType::U512
                | CLType::Unit
                | CLType::String
                | CLType::Key
                | CLType::URef
                | CLType::Option(_)
                | CLType::List(_)
                | CLType::ByteArray(..)
                | CLType::Result { .. }
                | CLType::Map { .. }
                | CLType::Tuple1(_)
                | CLType::Tuple2(_)
                | CLType::Tuple3(_)
                | CLType::PublicKey
                | CLType::Any => (),
            }
        };

        prop_oneof![
            Just((CLValue::from_t(()).expect("should create CLValue"), vec![])),
            any::<bool>()
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            any::<i32>().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            any::<i64>().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            any::<u8>().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            any::<u32>().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            any::<u64>().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            u128_arb().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            u256_arb().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            u512_arb().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            key_arb().prop_map(|x| {
                let urefs = x.as_uref().into_iter().cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            uref_arb().prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![x])),
            ".*".prop_map(|x: String| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            option::of(any::<u64>())
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            option::of(uref_arb()).prop_map(|x| {
                let urefs = x.iter().cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            option::of(key_arb()).prop_map(|x| {
                let urefs = x.iter().filter_map(Key::as_uref).cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            vec(any::<i32>(), 0..100)
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            vec(uref_arb(), 0..100).prop_map(|x| (
                CLValue::from_t(x.clone()).expect("should create CLValue"),
                x
            )),
            vec(key_arb(), 0..100).prop_map(|x| (
                CLValue::from_t(x.clone()).expect("should create CLValue"),
                x.into_iter().filter_map(Key::into_uref).collect()
            )),
            uniform32(any::<u8>())
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            result::maybe_err(key_arb(), ".*").prop_map(|x| {
                let urefs = match &x {
                    Ok(key) => key.as_uref().into_iter().cloned().collect(),
                    Err(_) => vec![],
                };
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            result::maybe_ok(".*", uref_arb()).prop_map(|x| {
                let urefs = match &x {
                    Ok(_) => vec![],
                    Err(uref) => vec![*uref],
                };
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            btree_map(".*", u512_arb(), 0..100)
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            btree_map(uref_arb(), u512_arb(), 0..100).prop_map(|x| {
                let urefs = x.keys().cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            btree_map(".*", uref_arb(), 0..100).prop_map(|x| {
                let urefs = x.values().cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            btree_map(uref_arb(), key_arb(), 0..100).prop_map(|x| {
                let mut urefs: Vec<URef> = x.keys().cloned().collect();
                urefs.extend(x.values().filter_map(Key::as_uref).cloned());
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            (any::<bool>())
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            (uref_arb())
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![x])),
            (any::<bool>(), any::<i32>())
                .prop_map(|x| (CLValue::from_t(x).expect("should create CLValue"), vec![])),
            (uref_arb(), any::<i32>()).prop_map(|x| {
                let uref = x.0;
                (
                    CLValue::from_t(x).expect("should create CLValue"),
                    vec![uref],
                )
            }),
            (any::<i32>(), key_arb()).prop_map(|x| {
                let urefs = x.1.as_uref().into_iter().cloned().collect();
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
            (uref_arb(), key_arb()).prop_map(|x| {
                let mut urefs = vec![x.0];
                urefs.extend(x.1.as_uref().into_iter().cloned());
                (CLValue::from_t(x).expect("should create CLValue"), urefs)
            }),
        ]
    }

    proptest! {
        #[test]
        fn should_extract_urefs((cl_value, urefs) in cl_value_with_urefs_arb()) {
            let extracted_urefs = extract_urefs(&cl_value).unwrap();
            assert_eq!(extracted_urefs, urefs);
        }
    }

    #[test]
    fn extract_from_public_keys_to_urefs_map() {
        let uref = URef::new([43; 32], AccessRights::READ_ADD_WRITE);
        let mut map = BTreeMap::new();
        map.insert(
            PublicKey::from(
                &SecretKey::ed25519_from_bytes([42; SecretKey::ED25519_LENGTH]).unwrap(),
            ),
            uref,
        );
        let cl_value = CLValue::from_t(map).unwrap();
        assert_eq!(extract_urefs(&cl_value).unwrap(), vec![uref]);
    }

    #[test]
    fn extract_from_public_keys_to_uref_keys_map() {
        let uref = URef::new([43; 32], AccessRights::READ_ADD_WRITE);
        let key = Key::from(uref);
        let mut map = BTreeMap::new();
        map.insert(
            PublicKey::from(
                &SecretKey::ed25519_from_bytes([42; SecretKey::ED25519_LENGTH]).unwrap(),
            ),
            key,
        );
        let cl_value = CLValue::from_t(map).unwrap();
        assert_eq!(extract_urefs(&cl_value).unwrap(), vec![uref]);
    }
}
