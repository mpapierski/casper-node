use std::collections::BTreeMap;

use casper_engine_test_support::{
    internal::{
        ExecuteRequestBuilder, InMemoryWasmTestBuilder, LmdbWasmTestBuilder, UpgradeRequestBuilder,
        DEFAULT_ACCOUNT_PUBLIC_KEY, DEFAULT_RUN_GENESIS_REQUEST,
    },
    AccountHash, DEFAULT_ACCOUNT_ADDR, MINIMUM_ACCOUNT_CREATION_BALANCE,
};
use casper_execution_engine::{core::{
        engine_state::{Error, SystemContractRegistry},
        execution,
    }, shared::{TypeMismatch, newtypes::{Blake2bHash, CorrelationId}, stored_value::StoredValue}};
use casper_types::{AccessRights, CLType, CLTyped, CLValue, ContractHash, ContractPackageHash, EraId, Key, ProtocolVersion, RuntimeArgs, U512, URef, bytesrepr::{Bytes, FromBytes, ToBytes}, runtime_args, system::{auction, auction::DelegationRate, mint}};

use crate::lmdb_fixture;

const ACCOUNT_1_ADDR: AccountHash = AccountHash::new([1u8; 32]);
const GH_1470_REGRESSION: &str = "gh_1470_regression.wasm";
const GH_1470_REGRESSION_CALL: &str = "gh_1470_regression_call.wasm";
const DEFAULT_ACTIVATION_POINT: EraId = EraId::new(1);

const CONTRACT_ADD_BID: &str = "add_bid.wasm";
const BOND_AMOUNT: u64 = 42;
const BID_DELEGATION_RATE: DelegationRate = auction::DELEGATION_RATE_DENOMINATOR;

const CONTRACT_GOV_25: &str = "gov_25.wasm";

struct Foo(Vec<u8>);

impl FromBytes for Foo {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), casper_types::bytesrepr::Error> {
        Ok((Foo(bytes.to_vec()), &[]))
    }
}

impl CLTyped for Foo {
    fn cl_type() -> casper_types::CLType {
        CLType::Any
    }
}

const EXPECTED_SIZE: usize = 2 * 1024 * 1024;
const CL_VALUE_OVERHEAD: usize = 5; // 4b length prefix + 1b cltype bytes

#[test]
fn gov25() {
    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&*DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_GOV_25,
        RuntimeArgs::default(),
    )
    .build();
    builder.exec(exec_request).expect_success().commit();



    let query_result = builder
        .query(
            None,
            Key::Account(*DEFAULT_ACCOUNT_ADDR),
            &["saved_0".to_string()],
        )
        .unwrap();
    let cl_value = query_result.as_cl_value().unwrap();
    let bytes: Foo = cl_value.clone().into_t().unwrap();
    assert_eq!(bytes.0.len(), EXPECTED_SIZE - CL_VALUE_OVERHEAD);
}
