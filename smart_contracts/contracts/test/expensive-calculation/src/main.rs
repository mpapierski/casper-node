#![no_std]
#![no_main]

extern crate alloc;

use casper_contract::{
    contract_api::{runtime, storage},
    unwrap_or_revert::UnwrapOrRevert,
};
use casper_types::{
    contracts::{NamedKeys, Parameters},
    CLType, EntryPoint, EntryPointAccess, EntryPointType, EntryPoints, Key,
};

const ENTRY_FUNCTION_NAME: &str = "calculate";

fn do_expensive_calculation() -> u64 {
    let large_prime: u64 = 0xffff_fffb;

    let mut result: u64 = 42;
    // calculate 42^4242 mod large_prime
    for _ in 1..4242 {
        result *= 42;
        result %= large_prime;
    }

    result
}

#[no_mangle]
pub extern "C" fn calculate() {
    let value = runtime::get_key("value")
        .and_then(Key::into_uref)
        .unwrap_or_revert();

    let result = do_expensive_calculation();
    storage::write(value, result);
}

#[no_mangle]
pub extern "C" fn call() {
    let entry_points = {
        let mut entry_points = EntryPoints::new();
        let entry_point = EntryPoint::new(
            ENTRY_FUNCTION_NAME,
            Parameters::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        entry_points
    };

    let named_keys = {
        let mut named_keys = NamedKeys::new();
        named_keys.insert("value".into(), storage::new_uref(0u64).into());
        named_keys
    };

    let (contract_hash, contract_version) =
        storage::new_contract(entry_points, Some(named_keys), None, None);
    runtime::put_key(
        "contract_version",
        storage::new_uref(contract_version).into(),
    );
    runtime::put_key("expensive-calculation", contract_hash.into());
}
