#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use borsh::{maybestd::io, BorshDeserialize, BorshSchema, BorshSerialize};
use casper_contract::{
    contract_api::{runtime, storage},
    unwrap_or_revert::UnwrapOrRevert,
};
use casper_types::{
    bytesrepr::{self, Bytes, ToBytes},
    contracts::{EntryPoint, EntryPoints, NamedKeys},
    CLType, CLTyped, EntryPointAccess, EntryPointType, Parameter, U512,
};

pub mod u512_capnp {
    include!(concat!(env!("OUT_DIR"), "/u512_capnp.rs"));
}

const ENTRY_FUNCTION_NAME: &str = "write_capnp";
const HASH_KEY_NAME: &str = "do_nothing_hash";
const PACKAGE_HASH_KEY_NAME: &str = "do_nothing_package_hash";
const ACCESS_KEY_NAME: &str = "do_nothing_access";
const CONTRACT_VERSION: &str = "contract_version";
const ARG_PURSE_NAME: &str = "purse_name";

const RANDOM_U512: U512 = U512([20, 67, 64, 96, 209, 102, 99, 158]);

// #[derive(BorshDeserialize)]
struct BorshU512<'a>(&'a U512);

impl<'a> BorshSerialize for BorshU512<'a> {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        // let U512(array) = self.0;
        let mut buf = [0u8; 64];
        self.0.to_little_endian(&mut buf);

        let mut skip_zeros = buf.len() - 1;
        while skip_zeros >= 0 && buf[skip_zeros] == 0 {
            skip_zeros -= 1;
        }
        let length_prefix = (skip_zeros + 1) as u8;
        writer.write_all(&[length_prefix])?;
        writer.write_all(&buf[..skip_zeros + 1])?;
        Ok(())
    }
}

use capnp::serialize_packed;
use u512_capnp::u512;

pub fn write_u512_capnp(number: U512) -> ::capnp::Result<Vec<u8>> {
    let mut message = ::capnp::message::Builder::new_default();
    {
        let U512(array) = number;

        let mut u512 = message.init_root::<u512::Builder>();

        u512.set_a(array[0]);
        u512.set_b(array[1]);
        u512.set_c(array[2]);
        u512.set_d(array[3]);
        u512.set_e(array[4]);
        u512.set_f(array[5]);
        u512.set_g(array[6]);
        u512.set_h(array[7]);
    }

    let mut vec = Vec::new();
    serialize_packed::write_message(&mut vec, &message)?;
    Ok(vec)
}

#[no_mangle]
pub extern "C" fn write_capnp() {
    let uref = runtime::get_key("storage")
        .unwrap_or_revert()
        .into_uref()
        .unwrap_or_revert();
    let data = write_u512_capnp(RANDOM_U512).ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}

#[no_mangle]
pub extern "C" fn write_borsh() {
    let uref = runtime::get_key("storage")
        .unwrap_or_revert()
        .into_uref()
        .unwrap_or_revert();
    let u512_value = BorshU512(&RANDOM_U512);
    let data = u512_value.try_to_vec().ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}

#[no_mangle]
pub extern "C" fn write_tobytes() {
    let uref = runtime::get_key("storage")
        .unwrap_or_revert()
        .into_uref()
        .unwrap_or_revert();
    let u512_value = RANDOM_U512;
    let data = u512_value.to_bytes().ok().unwrap_or_revert();
    storage::write(uref, Bytes::from(data));
}

#[no_mangle]
pub extern "C" fn call() {
    let entry_points = {
        let mut entry_points = EntryPoints::new();
        let entry_point = EntryPoint::new(
            ENTRY_FUNCTION_NAME.to_string(),
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        let entry_point = EntryPoint::new(
            "write_borsh",
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        let entry_point = EntryPoint::new(
            "write_tobytes",
            Vec::new(),
            CLType::Unit,
            EntryPointAccess::Public,
            EntryPointType::Contract,
        );
        entry_points.add_entry_point(entry_point);
        entry_points
    };

    let named_keys = {
        let uref = storage::new_uref(());
        let mut named_keys = NamedKeys::new();
        named_keys.insert("storage".to_string(), uref.into());

        named_keys
    };

    let (contract_hash, contract_version) = storage::new_contract(
        entry_points,
        Some(named_keys),
        Some(PACKAGE_HASH_KEY_NAME.to_string()),
        Some(ACCESS_KEY_NAME.to_string()),
    );

    runtime::put_key(CONTRACT_VERSION, storage::new_uref(contract_version).into());
    runtime::put_key(HASH_KEY_NAME, contract_hash.into());
}
