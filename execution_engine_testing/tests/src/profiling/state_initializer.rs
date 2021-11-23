//! This executable is designed to be run to set up global state in preparation for running other
//! standalone test executable(s).  This will allow profiling to be done on executables running only
//! meaningful code, rather than including test setup effort in the profile results.

use std::{env, path::PathBuf, io::Write, fs};

use clap::{crate_version, App};

use casper_engine_test_support::internal::{
    DeployItemBuilder, ExecuteRequestBuilder, LmdbWasmTestBuilder, ARG_AMOUNT, DEFAULT_ACCOUNTS,
    DEFAULT_ACCOUNT_ADDR, DEFAULT_AUCTION_DELAY, DEFAULT_GENESIS_CONFIG_HASH,
    DEFAULT_GENESIS_TIMESTAMP_MILLIS, DEFAULT_LOCKED_FUNDS_PERIOD_MILLIS, DEFAULT_PAYMENT,
    DEFAULT_PROTOCOL_VERSION, DEFAULT_ROUND_SEIGNIORAGE_RATE, DEFAULT_SYSTEM_CONFIG,
    DEFAULT_UNBONDING_DELAY, DEFAULT_VALIDATOR_SLOTS, DEFAULT_WASM_CONFIG,
};
use casper_engine_tests::profiling;
use casper_execution_engine::core::engine_state::{
    engine_config::EngineConfig, genesis::ExecConfig, run_genesis_request::RunGenesisRequest,
};
use casper_types::{runtime_args, RuntimeArgs};

const ABOUT: &str = "Initializes global state in preparation for profiling runs. Outputs the root \
                     hash from the commit response.";
const STATE_INITIALIZER_CONTRACT: &str = "gh_2346_regression.wasm";

fn data_dir() -> PathBuf {
    let exe_name = profiling::exe_name();
    let data_dir_arg = profiling::data_dir_arg();
    let arg_matches = App::new(&exe_name)
        .version(crate_version!())
        .about(ABOUT)
        .arg(data_dir_arg)
        .get_matches();
    profiling::data_dir(&arg_matches)
}

fn main() {
    let data_dir = data_dir();

    let genesis_account_hash = *DEFAULT_ACCOUNT_ADDR;

    let exec_request_1 = {
        let deploy = DeployItemBuilder::new()
            .with_address(*DEFAULT_ACCOUNT_ADDR)
            .with_deploy_hash([1; 32])
            .with_session_code(
                STATE_INITIALIZER_CONTRACT,
                RuntimeArgs::default(),
            )
            .with_empty_payment_bytes(runtime_args! { ARG_AMOUNT => *DEFAULT_PAYMENT, })
            .with_authorization_keys(&[genesis_account_hash])
            .build();

        ExecuteRequestBuilder::new().push_deploy(deploy).build()
    };

    // let exec_request_2 = {
    //     let deploy = DeployItemBuilder::new()
    //         .with_address(*DEFAULT_ACCOUNT_ADDR)
    //         .with_deploy_hash([2; 32])
    //         .with_stored_session_named_key("contract_hash", "create_domains", runtime_args! {
    //             "number" => 50_000u64,
    //         })
    //         // .with_session_code(
    //         //     "simple_transfer.wasm",
    //         //     runtime_args! { "target" =>account_2_account_hash, "amount" => U512::from(TRANSFER_AMOUNT) },
    //         // )
    //         .with_empty_payment_bytes( runtime_args! { "amount" => (*DEFAULT_PAYMENT * 10)})
    //         .with_authorization_keys(&[*DEFAULT_ACCOUNT_ADDR])
    //         .build();

    //     ExecuteRequestBuilder::new().push_deploy(deploy).build()
    // };

    let engine_config = EngineConfig::default();
    let mut builder = LmdbWasmTestBuilder::new_with_config(&data_dir, engine_config);

    let exec_config = ExecConfig::new(
        DEFAULT_ACCOUNTS.clone(),
        *DEFAULT_WASM_CONFIG,
        *DEFAULT_SYSTEM_CONFIG,
        DEFAULT_VALIDATOR_SLOTS,
        DEFAULT_AUCTION_DELAY,
        DEFAULT_LOCKED_FUNDS_PERIOD_MILLIS,
        DEFAULT_ROUND_SEIGNIORAGE_RATE,
        DEFAULT_UNBONDING_DELAY,
        DEFAULT_GENESIS_TIMESTAMP_MILLIS,
    );
    let run_genesis_request = RunGenesisRequest::new(
        *DEFAULT_GENESIS_CONFIG_HASH,
        *DEFAULT_PROTOCOL_VERSION,
        exec_config,
    );

    builder
        .run_genesis(&run_genesis_request);

    builder
        .exec(exec_request_1)
        .expect_success()
        .commit();

    // builder
    //     .exec(exec_request_2)
    //     .expect_success()
    //     .commit();

    let post_state_hash = builder
        .get_post_state_hash();
    println!("{}", base16::encode_lower(&post_state_hash));

    fs::write("state_hash.raw", &post_state_hash).unwrap();
}
