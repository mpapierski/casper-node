use once_cell::sync::Lazy;
use rand::Rng;
use tempfile::TempDir;

use casper_engine_test_support::{
    ExecuteRequestBuilder, InMemoryWasmTestBuilder, LmdbWasmTestBuilder, UpgradeRequestBuilder,
    DEFAULT_ACCOUNT_ADDR, DEFAULT_EXEC_CONFIG, DEFAULT_GENESIS_CONFIG_HASH,
    DEFAULT_PROTOCOL_VERSION, DEFAULT_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::core::engine_state::{
    ChainspecRegistry, EngineConfig, RunGenesisRequest,
};
use casper_hashing::Digest;
use casper_types::{ContractHash, EraId, Key, ProtocolVersion, RuntimeArgs, StoredValue};

use crate::lmdb_fixture;

const DEFAULT_ACTIVATION_POINT: EraId = EraId::new(1);

static OLD_PROTOCOL_VERSION: Lazy<ProtocolVersion> = Lazy::new(|| *DEFAULT_PROTOCOL_VERSION);
static NEW_PROTOCOL_VERSION: Lazy<ProtocolVersion> = Lazy::new(|| {
    ProtocolVersion::from_parts(
        OLD_PROTOCOL_VERSION.value().major,
        OLD_PROTOCOL_VERSION.value().minor,
        OLD_PROTOCOL_VERSION.value().patch + 1,
    )
});

#[ignore]
#[test]
fn should_execute_legacy_contract_struct() {
    let (mut builder, _lmdb_fixture_state, _temp_dir) =
        lmdb_fixture::builder_from_global_state_fixture("contract_upgrade");

    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have account");
    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();

    let contract = builder.query(None, Key::from(contract_hash), &[]).unwrap();

    assert!(matches!(contract, StoredValue::Contract(_v1)));

    let exec_request = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "do_nothing_entrypoint",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    // Not modified
    let contract = builder.query(None, Key::from(contract_hash), &[]).unwrap();
    assert!(matches!(contract, StoredValue::Contract(_v1)));

    let exec_request = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "put_key_entrypoint",
        RuntimeArgs::default(),
    )
    .with_block_time(1)
    .build();

    builder.exec(exec_request).expect_success().commit();

    // Modified
    let contract = builder.query(None, Key::from(contract_hash), &[]).unwrap();
    let v2 = match contract {
        StoredValue::Contract(_) => panic!("should be upgraded"),
        StoredValue::ContractV2(v2) => v2,
        _ => panic!("unexpected variant"),
    };

    assert_eq!(v2.contract_hash(), Some(contract_hash));

    // Execute v2
    let exec_request = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "do_nothing_entrypoint",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    // Modified
    let contract = builder.query(None, Key::from(contract_hash), &[]).unwrap();
    let v2 = match contract {
        StoredValue::Contract(_) => panic!("should be upgraded"),
        StoredValue::ContractV2(v2) => v2,
        _ => panic!("unexpected variant"),
    };

    assert_eq!(v2.contract_hash(), Some(contract_hash));
}
