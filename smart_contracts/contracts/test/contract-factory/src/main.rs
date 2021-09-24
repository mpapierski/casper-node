#![no_std]
#![no_main]

extern crate alloc;

use casper_contract::contract_api::{runtime, storage};
use casper_types::{
    contracts::{NamedKeys, Parameters},
    CLType, CLTyped, EntryPoint, EntryPointAccess, EntryPointType, EntryPoints,
};

#[no_mangle]
pub extern "C" fn do_something() {}

#[no_mangle]
pub extern "C" fn factory() {
    let mut entry_points = EntryPoints::new();
    entry_points.add_entry_point(EntryPoint::new(
        "do_something",
        Parameters::new(),
        <()>::cl_type(),
        EntryPointAccess::Public,
        EntryPointType::Contract,
    ));

    let (contract_package_hash, access_uref) = storage::create_contract_package_at_hash();
    let named_keys = NamedKeys::new();
    let (contract_hash, _version) =
        storage::add_contract_version(contract_package_hash, entry_points, named_keys);
    runtime::put_key("created_contract_hash", contract_hash.into());
}

#[no_mangle]
pub extern "C" fn call() {
    let mut entry_points = EntryPoints::new();
    entry_points.add_entry_point(EntryPoint::new(
        "factory",
        Parameters::new(),
        <()>::cl_type(),
        EntryPointAccess::Public,
        EntryPointType::Contract,
    ));

    let (contract_package_hash, access_uref) = storage::create_contract_package_at_hash();

    let named_keys = {
        let mut named_keys = NamedKeys::new();
        named_keys
    };

    let (contract_hash, _version) =
        storage::add_contract_version(contract_package_hash, entry_points, named_keys);
    runtime::put_key("contract_hash", contract_hash.into());
}
