use std::time::Instant;

use num_traits::Zero;

use casper_engine_test_support::{
    internal::{
        DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder,
        DEFAULT_RUN_GENESIS_REQUEST,
    },
    DEFAULT_ACCOUNT_ADDR,
};
use casper_execution_engine::{core::{engine_state::{self, Error as CoreError, MAX_PAYMENT}, execution}, shared::{opcode_costs::DEFAULT_NOP_COST, wasm}};
use casper_types::{contracts::DEFAULT_ENTRY_POINT_NAME, runtime_args, Gas, RuntimeArgs, U512};
use parity_wasm::{
    builder,
    elements::{Instruction, Instructions},
};

const ARG_AMOUNT: &str = "amount";

#[ignore]
#[test]
fn should_run_endless_loop() {
    let exec = ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, "endless_loop.wasm", RuntimeArgs::default()).build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    let start =Instant::now();
    builder.exec(exec).commit();
    let stop = start.elapsed();

    let maybe_error = builder.get_error();
    assert!(matches!(maybe_error, Some(engine_state::Error::Exec(execution::Error::GasLimit))), "{:?}", maybe_error);
    eprintln!("elapsed {:?}", stop);
}

#[ignore]
#[test]
fn should_run_create_accounts() {
    let exec = ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, "create_accounts.wasm", RuntimeArgs::default()).build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    builder.exec(exec).expect_success().commit();
}
