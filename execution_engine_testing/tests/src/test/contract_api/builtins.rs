use casper_types::{runtime_args, system::standard_payment::ARG_AMOUNT};
use rand::Rng;

use casper_engine_test_support::{
    DeployItemBuilder, ExecuteRequestBuilder, LmdbWasmTestBuilder, DEFAULT_ACCOUNT_ADDR,
    DEFAULT_PAYMENT, LOCAL_GENESIS_REQUEST,
};

const ALTBN128_WASM: &str = "altbn128.wasm";

#[ignore]
#[test]
fn altbn128_builtins_should_work() {
    let mut builder = LmdbWasmTestBuilder::default();
    builder.run_genesis(LOCAL_GENESIS_REQUEST.clone());

    let mut rng = rand::thread_rng();
    let deploy_hash = rng.gen();
    let address = *DEFAULT_ACCOUNT_ADDR;
    let deploy_item = DeployItemBuilder::new()
        .with_address(address)
        .with_session_code(ALTBN128_WASM, runtime_args! {})
        .with_standard_payment(runtime_args! {
            ARG_AMOUNT => *DEFAULT_PAYMENT
        })
        .with_authorization_keys(&[address])
        .with_deploy_hash(deploy_hash)
        .build();
    let execute_request = ExecuteRequestBuilder::from_deploy_item(&deploy_item).build();

    builder.exec(execute_request).commit().expect_success();

    // Compile above test contract with either feature flags: wasm_add_test, wasm_mul_test,
    // wasm_pairing_test, and uncomment gas line. None of these flags will use host functions.
    // bash -c 'cd smart_contracts/contracts &&
    // RUSTFLAGS="--remap-path-prefix=$HOME/.cargo= --remap-path-prefix=$PWD=/dir" cargo --locked
    // build --verbose --release --package altbn128 --features wasm_add_test'

    // eprintln!("gas {}", builder.last_exec_gas_cost());
}
