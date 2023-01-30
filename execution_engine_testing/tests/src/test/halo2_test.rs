use std::fs;

use casper_types::{bytesrepr::Bytes, system::mint};
use once_cell::sync::Lazy;

use casper_engine_test_support::{
    ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNT_ADDR,
    DEFAULT_ACCOUNT_PUBLIC_KEY, MINIMUM_ACCOUNT_CREATION_BALANCE, PRODUCTION_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::core::{
    engine_state, engine_state::engine_config::DEFAULT_MINIMUM_DELEGATION_AMOUNT, execution,
};
use casper_types::{
    self,
    account::AccountHash,
    api_error::ApiError,
    runtime_args,
    system::auction::{
        DelegationRate, ARG_AMOUNT, ARG_DELEGATION_RATE, ARG_DELEGATOR, ARG_PUBLIC_KEY,
        ARG_VALIDATOR,
    },
    PublicKey, RuntimeArgs, SecretKey, U512,
};

#[test]
fn should_fail_to_add_new_bid_over_the_approved_amount() {
    let proof = fs::read("/tmp/proof.bin").unwrap();
    let params = fs::read("/tmp/params.bin").unwrap();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "halo2_test.wasm",
        runtime_args! {
            "proof" => Bytes::from(proof),
            "params" => Bytes::from(params),
        },
    )
    .build();
    builder.exec(exec_request).expect_success().commit();
    println!("gas {}", builder.last_exec_gas_cost());
}
