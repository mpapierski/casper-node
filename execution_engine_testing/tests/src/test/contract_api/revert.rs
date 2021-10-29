use std::time::Instant;

use casper_engine_test_support::{
    internal::{ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_RUN_GENESIS_REQUEST},
    DEFAULT_ACCOUNT_ADDR,
};
use casper_execution_engine::core::{engine_state, execution};
use casper_types::{ApiError, RuntimeArgs};

const REVERT_WASM: &str = "revert.wasm";

#[ignore]
#[test]
fn should_revert() {
    let exec_request =
        ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, REVERT_WASM, RuntimeArgs::default())
            .build();
    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    let start = Instant::now();
    builder.exec(exec_request);
    let stop = start.elapsed();
    builder.commit();

    let maybe_error = builder.get_error();
    match maybe_error {
        Some(engine_state::Error::Exec(execution::Error::Revert(api_error)))
            if api_error == ApiError::User(100) => {}
        _ => panic!("should be an error but {:?}", maybe_error),
    }
}
