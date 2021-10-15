#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::vec::Vec;
use casper_contract::{
    contract_api::{self, account, runtime, storage},
    ext_ffi,
};
use casper_types::{bytesrepr::Bytes, U512};

#[no_mangle]
pub extern "C" fn call() {
    let uref = storage::new_uref(());

    let mut value: U512 = U512::one();

    loop {
        let _ = account::get_main_purse();
        let mut data: Vec<u8> = vec![0u8; 4096].into();

        value.to_big_endian(&mut data);
        value += U512::one();
        runtime::print(&format!("value: {}", value));

        storage::write(uref, Bytes::from(data));
    }

    assert!(false);
}
