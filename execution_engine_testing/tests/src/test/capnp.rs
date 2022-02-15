use num_traits::Zero;

use casper_engine_test_support::{
    DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNT_ADDR,
    DEFAULT_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::{
    core::engine_state::{Error as CoreError, MAX_PAYMENT},
    shared::{opcode_costs::DEFAULT_NOP_COST, wasm},
};
use casper_types::{contracts::DEFAULT_ENTRY_POINT_NAME, runtime_args, Gas, RuntimeArgs, U512};
use parity_wasm::{
    builder,
    elements::{Instruction, Instructions},
};

const ARG_AMOUNT: &str = "amount";

#[ignore]
#[test]
fn measure_capnp() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, "serialization_stored.wasm", RuntimeArgs::default()).build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(*DEFAULT_ACCOUNT_ADDR, "do_nothing_hash", "write_capnp", RuntimeArgs::default()).build();

    builder.exec(exec_request_2).expect_success().commit();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(gas, Gas::zero());
}


#[ignore]
#[test]
fn measure_borsh() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, "serialization_stored.wasm", RuntimeArgs::default()).build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(*DEFAULT_ACCOUNT_ADDR, "do_nothing_hash", "write_borsh", RuntimeArgs::default()).build();

    builder.exec(exec_request_2).expect_success().commit();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(gas, Gas::zero());
}

#[ignore]
#[test]
fn measure_tobytes() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, "serialization_stored.wasm", RuntimeArgs::default()).build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(*DEFAULT_ACCOUNT_ADDR, "do_nothing_hash", "write_tobytes", RuntimeArgs::default()).build();

    builder.exec(exec_request_2).expect_success().commit();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(gas, Gas::zero());
}
