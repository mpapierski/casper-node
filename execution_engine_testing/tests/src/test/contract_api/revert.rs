use std::time::Instant;

use casper_engine_test_support::{
    instrumented, ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNT_ADDR,
    PRODUCTION_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::core::{
    engine_state::{self, Error},
    execution,
};
use casper_types::{ApiError, RuntimeArgs};

const REVERT_WASM: &str = "revert.wasm";

#[ignore]
#[test]
fn should_revert() {
    let exec_request =
        ExecuteRequestBuilder::standard(*DEFAULT_ACCOUNT_ADDR, REVERT_WASM, RuntimeArgs::default())
            .build();
    let mut builder = InMemoryWasmTestBuilder::default();

    builder
        .run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST)
        .exec_instrumented(instrumented!(exec_request))
        .commit();

    let error = builder.get_error().expect("should have error");

    assert!(
        matches!(error, Error::Exec(execution::Error::Revert(ApiError::User(
            user_error,
        ))) if user_error == 100),
        "{:?}",
        error
    );
}
