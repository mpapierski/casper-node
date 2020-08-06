#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

use casperlabs_contract::contract_api::{account, runtime, system};

use casperlabs_types::{auction::{ARG_ACCOUNT_HASH, DelegationRate, ARG_SOURCE_PURSE}, runtime_args, ApiError, RuntimeArgs, URef, U512};

const ARG_ENTRY_POINT: &str = "entry_point";
const ARG_ADD_BID: &str = "add_bid";
const ARG_WITHDRAW_BID: &str = "withdraw_bid";
const ARG_AMOUNT: &str = "amount";
const ARG_DELEGATION_RATE: &str = "delegation_rate";

#[repr(u16)]
enum Error {
    UnknownCommand,
}

#[no_mangle]
pub extern "C" fn call() {
    let command: String = runtime::get_named_arg(ARG_ENTRY_POINT);

    match command.as_str() {
        ARG_ADD_BID => add_bid(),
        ARG_WITHDRAW_BID => withdraw_bid(),
        _ => runtime::revert(ApiError::User(Error::UnknownCommand as u16)),
    }
}

fn add_bid() {
    let auction = system::get_auction();
    let quantity: U512 = runtime::get_named_arg(ARG_AMOUNT);
    let delegation_rate: DelegationRate = runtime::get_named_arg(ARG_DELEGATION_RATE);

    let args = runtime_args! {
        ARG_ACCOUNT_HASH => runtime::get_caller(),
        ARG_SOURCE_PURSE => account::get_main_purse(),
        ARG_DELEGATION_RATE => delegation_rate,
        ARG_AMOUNT => quantity,
    };

    let (_purse, _quantity): (URef, U512) = runtime::call_contract(auction, "add_bid", args);
}

fn withdraw_bid() {
    let auction = system::get_auction();
    let quantity: U512 = runtime::get_named_arg(ARG_AMOUNT);

    let args = runtime_args! {
        ARG_AMOUNT => quantity,
        ARG_ACCOUNT_HASH => runtime::get_caller(),
    };

    let (_purse, _quantity): (URef, U512) = runtime::call_contract(auction, "withdraw_bid", args);
}