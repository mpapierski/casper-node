use casper_engine_test_support::{
    ExecuteRequestBuilder, WasmTestBuilder, DEFAULT_ACCOUNT_ADDR, DEFAULT_RUN_GENESIS_REQUEST,
};
use casper_types::{bytesrepr::Bytes, Key, RuntimeArgs};

const CONTRACT_BIG_TRIE_LEAF: &str = "big_trie_leaf.wasm";
const KB: usize = 1024;
const MB: usize = 1024 * KB;
const DATA_SIZE: usize = 2 * MB;
const KEY_NAME: &str = "data";
const BLOCK_TIME: u64 = u64::MAX - 1;

#[ignore]
#[test]
fn should_run_mint_purse_contract() {
    let exec_request_1 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_BIG_TRIE_LEAF,
        RuntimeArgs::new(),
    )
    .with_block_time(BLOCK_TIME)
    .build();

    let mut builder = WasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    builder.exec(exec_request_1).commit().expect_success();

    let stored = builder
        .query(
            None,
            Key::Account(*DEFAULT_ACCOUNT_ADDR),
            &[KEY_NAME.to_string()],
        )
        .unwrap();

    let cl_value = stored.as_cl_value().expect("should store cl value");
    let bytes: Bytes = cl_value.clone().into_t().expect("should have u8 array");
    assert_eq!(bytes.len(), DATA_SIZE);
    assert_ne!(bytes, Bytes::from(vec![0; DATA_SIZE]));
    assert_eq!(bytes[0..8], BLOCK_TIME.to_be_bytes());
}
