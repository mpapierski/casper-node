#![no_main]

use casper_contract::{contract_api::storage, unwrap_or_revert::UnwrapOrRevert};

#[no_mangle]
pub extern "C" fn call() {
    let seed_uref = storage::new_dictionary("nctl_dictionary").unwrap_or_revert();
    storage::dictionary_put(seed_uref, "foo", 1u64);
}
