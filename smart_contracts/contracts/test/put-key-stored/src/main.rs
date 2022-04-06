#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::string::ToString;

use casper_contract::contract_api::{runtime, storage, system};
use casper_types::{
    contracts::{EntryPoint, EntryPoints},
    CLType, EntryPointAccess, EntryPointType, Key,
};

const PUT_KEY_ENTRYPOINT: &str = "put_key_entrypoint";
const DO_NOTHING_ENTRYPOINT: &str = "do_nothing_entrypoint";
const HASH_KEY_NAME: &str = "do_nothing_hash";
const PACKAGE_HASH_KEY_NAME: &str = "do_nothing_package_hash";
const ACCESS_KEY_NAME: &str = "do_nothing_access";
const CONTRACT_VERSION: &str = "contract_version";

#[no_mangle]
pub extern "C" fn put_key_entrypoint() {
    // This entrypoit should force new Contract structure to be written to global state.
    let blocktime = runtime::get_blocktime();
    let blocktime_u64: u64 = blocktime.into();
    runtime::put_key(&format!("key{}", blocktime_u64), Key::Hash([0; 32]));
}
#[no_mangle]
pub extern "C" fn do_nothing_entrypoint() {
    // This entrypoint does not overwrite Contract structure.
    let _ = runtime::list_named_keys();
}

#[no_mangle]
pub extern "C" fn call() {
    let entry_points = {
        let mut entry_points = EntryPoints::new();
        let entry_point = EntryPoint::new(
            PUT_KEY_ENTRYPOINT.to_string(),
            vec![],
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);

        let entry_point = EntryPoint::new(
            DO_NOTHING_ENTRYPOINT.to_string(),
            vec![],
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);

        entry_points
    };

    let (contract_hash, contract_version) = storage::new_contract(
        entry_points,
        None,
        Some(PACKAGE_HASH_KEY_NAME.to_string()),
        Some(ACCESS_KEY_NAME.to_string()),
    );

    runtime::put_key(CONTRACT_VERSION, storage::new_uref(contract_version).into());
    runtime::put_key(HASH_KEY_NAME, contract_hash.into());
}
