#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use borsh::{BorshDeserialize, BorshSerialize};
use casper_contract::{
    contract_api::{account, runtime, storage, system},
    unwrap_or_revert::UnwrapOrRevert,
};
use casper_types::{
    bytesrepr::{Error, FromBytes, ToBytes, U8_SERIALIZED_LENGTH},
    runtime_args, ApiError, CLType, CLTyped, ContractHash, ContractPackageHash, EntryPointType,
    Key, Phase, RuntimeArgs, Tagged, URef, U512,
};

pub const CONTRACT_PACKAGE_NAME: &str = "forwarder";
pub const PACKAGE_ACCESS_KEY_NAME: &str = "forwarder_access";
pub const CONTRACT_NAME: &str = "our_contract_name";

pub const METHOD_FORWARDER_CONTRACT_NAME: &str = "forwarder_contract";
pub const METHOD_FORWARDER_SESSION_NAME: &str = "forwarder_session";

pub const ARG_CALLS: &str = "calls";
pub const ARG_CURRENT_DEPTH: &str = "current_depth";

const DEFAULT_PAYMENT: u64 = 1_500_000_000_000;

#[repr(u8)]
enum ContractAddressTag {
    ContractHash = 0,
    ContractPackageHash,
}

#[derive(Debug, Copy, Clone, BorshSerialize, BorshDeserialize)]
pub enum ContractAddress {
    ContractHash(ContractHash),
    ContractPackageHash(ContractPackageHash),
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Call {
    pub contract_address: ContractAddress,
    pub target_method: String,
    pub entry_point_type: EntryPointType,
}

impl CLTyped for Call {
    fn cl_type() -> CLType {
        CLType::Any
    }
}

pub fn standard_payment(amount: U512) {
    const METHOD_GET_PAYMENT_PURSE: &str = "get_payment_purse";

    let main_purse = account::get_main_purse();

    let handle_payment_pointer = system::get_handle_payment();

    let payment_purse: URef = runtime::call_contract(
        handle_payment_pointer,
        METHOD_GET_PAYMENT_PURSE,
        RuntimeArgs::default(),
    );

    system::transfer_from_purse_to_purse(main_purse, payment_purse, amount, None).unwrap_or_revert()
}

pub fn recurse() {
    let calls: Vec<Call> = runtime::get_named_arg(ARG_CALLS);
    let current_depth: u8 = runtime::get_named_arg(ARG_CURRENT_DEPTH);

    // The important bit
    {
        let call_stack = runtime::get_call_stack();
        let name = alloc::format!("call_stack-{}", current_depth);
        let call_stack_at = storage::new_uref(call_stack);
        runtime::put_key(&name, Key::URef(call_stack_at));
    }

    if current_depth == 0 && runtime::get_phase() == Phase::Payment {
        standard_payment(U512::from(DEFAULT_PAYMENT))
    }

    if current_depth == calls.len() as u8 {
        return;
    }

    let args = runtime_args! {
        ARG_CALLS => calls.clone(),
        ARG_CURRENT_DEPTH => current_depth + 1u8,
    };

    match calls.get(current_depth as usize) {
        Some(Call {
            contract_address: ContractAddress::ContractPackageHash(contract_package_hash),
            target_method,
            ..
        }) => {
            runtime::call_versioned_contract::<()>(
                *contract_package_hash,
                None,
                target_method,
                args,
            );
        }
        Some(Call {
            contract_address: ContractAddress::ContractHash(contract_hash),
            target_method,
            ..
        }) => {
            runtime::call_contract::<()>(*contract_hash, target_method, args);
        }
        _ => runtime::revert(ApiError::User(0)),
    }
}
