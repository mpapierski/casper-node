#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use core::mem::{self, MaybeUninit};

use alloc::vec::Vec;

use casper_contract::{
    contract_api::{self, runtime, storage},
    ext_ffi,
};

use casper_types::{
    bytesrepr::ToBytes, contracts::NamedKeys, CLType, CLTyped, EntryPoint, EntryPointAccess,
    EntryPointType, EntryPoints, Group, Key, Parameter, URef,
};

/// Writes `value` under `uref` in the global state.
pub fn raw_write(uref: URef, value: &[u8]) {
    let key = Key::from(uref);
    let key_bytes = key.to_bytes().unwrap();

    unsafe {
        ext_ffi::casper_write(
            key_bytes.as_ptr(),
            key_bytes.len(),
            value.as_ptr(),
            value.len(),
        );
    }
}

const MB: usize = 1024 * 1024;
const PAYLOAD_LENGTH: usize = 1 * MB;

const CL_TYPE_TAG_ANY: u8 = 21;

const CL_TYPE_ANY_BYTES: [u8; 1] = [CL_TYPE_TAG_ANY];

const LENGTH_PREFIX_SIZE: usize = 4;

/// This array should exceed the heap memory limit.
const BIG_DATA_SIZE: usize = (4 * MB) + 1;
const REALLY_BIG_DATA: [u8; BIG_DATA_SIZE] = [0; (4 * MB) + 1];

#[no_mangle]
pub extern "C" fn call() {

    let cl_value_bytes = {
        let ptr = contract_api::alloc_bytes(PAYLOAD_LENGTH);
        let mut cl_value_bytes =
            unsafe { Vec::from_raw_parts(ptr.as_ptr(), PAYLOAD_LENGTH, PAYLOAD_LENGTH) };

        // Set up CLValue layout
        let payload_length = PAYLOAD_LENGTH - LENGTH_PREFIX_SIZE - CL_TYPE_ANY_BYTES.len();
        let length_prefix_bytes = payload_length.to_le_bytes();
        // First 4 bytes is the length prefix
        cl_value_bytes[0..LENGTH_PREFIX_SIZE].copy_from_slice(&length_prefix_bytes);
        // Last N bytes are bytes of CLType
        cl_value_bytes[PAYLOAD_LENGTH - CL_TYPE_ANY_BYTES.len()..].copy_from_slice(&CL_TYPE_ANY_BYTES);

        cl_value_bytes
    };

    for i in 0..2 {

        // let mut addr = "saved_
        let uref = storage::new_uref(());
        raw_write(uref, &cl_value_bytes);

        // runtime::put_key(&format!("saved_{}", i), uref.into());
    }
}
