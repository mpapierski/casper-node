#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::string::ToString;
use casper_contract::{
    contract_api::{runtime, storage},
    unwrap_or_revert::UnwrapOrRevert,
};

use casper_types::{
    contracts::NamedKeys, CLType, CLTyped, EntryPoint, EntryPointAccess, EntryPointType,
    EntryPoints, Key, Parameter,
};

const ARG_NUMBER: &str = "number";
const CREATE_DOMAINS_ENTRYPOINT: &str = "create_domains";
const CONTRACT_HASH_NAME: &str = "contract_hash";
const CONTRACT_PACKAGE_HASH_NAME: &str = "contract_package_hash";
const DOMAINS_DICT_NAME: &str = "domains";

#[no_mangle]
pub extern "C" fn create_domains() {
    let number: u64 = runtime::get_named_arg(ARG_NUMBER);
    // let n: u64 = runtime::get_named_arg(ARG_N);

    let dictionary_seed_uref = runtime::get_key(DOMAINS_DICT_NAME)
        .and_then(Key::into_uref)
        .unwrap_or_revert();

    for i in 500..number {
        // let dictionary_item_key = format!("{}-in-{}", n, i);
        let dictionary_item_key = format!("in-{}", i);
        let value = format!("domain {}", i);
        storage::dictionary_put(dictionary_seed_uref, &dictionary_item_key, value);
    }
}

#[no_mangle]
pub extern "C" fn call() {
    let (contract_package_hash, _access_uref) = storage::create_contract_package_at_hash();

    let mut entry_points = EntryPoints::new();

    entry_points.add_entry_point(EntryPoint::new(
        CREATE_DOMAINS_ENTRYPOINT,
        vec![
            Parameter::new(ARG_NUMBER, u64::cl_type()),
            // Parameter::new(ARG_N, u64::cl_type()),
        ],
        CLType::Unit,
        EntryPointAccess::Public,
        EntryPointType::Contract,
    ));

    let named_keys = {
        let mut named_keys = NamedKeys::new();
        let dict = storage::new_dictionary(DOMAINS_DICT_NAME).unwrap_or_revert();
        runtime::remove_key(DOMAINS_DICT_NAME);
        named_keys.insert(DOMAINS_DICT_NAME.to_string(), dict.into());
        named_keys
    };

    let (contract_hash, _) =
        storage::add_contract_version(contract_package_hash, entry_points, named_keys);

    runtime::put_key(CONTRACT_HASH_NAME, contract_hash.into());
    runtime::put_key(CONTRACT_PACKAGE_HASH_NAME, contract_package_hash.into());
}
