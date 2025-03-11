use std::{fmt::Write, fs};

use casper_wasm::{
    builder,
    elements::{Instruction, Instructions},
};

use casper_engine_test_support::{
    utils, DeployItemBuilder, ExecuteRequestBuilder, LmdbWasmTestBuilder, DEFAULT_ACCOUNTS,
    DEFAULT_ACCOUNT_ADDR, DEFAULT_ACCOUNT_PUBLIC_KEY, DEFAULT_PAYMENT, LOCAL_GENESIS_REQUEST,
    MINIMUM_ACCOUNT_CREATION_BALANCE,
};
use casper_execution_engine::{
    engine_state,
    execution::ExecError,
    runtime::{
        PreprocessingError, WasmValidationError, DEFAULT_BR_TABLE_MAX_SIZE, DEFAULT_MAX_GLOBALS,
        DEFAULT_MAX_PARAMETER_COUNT, DEFAULT_MAX_TABLE_SIZE,
    },
};
use casper_types::{
    account::AccountHash,
    addressable_entity::DEFAULT_ENTRY_POINT_NAME,
    bytesrepr::{Bytes, ToBytes},
    runtime_args,
    system::auction::{ARG_AMOUNT, ARG_DELEGATION_RATE, ARG_PUBLIC_KEY},
    GenesisAccount, Motes, PublicKey, RuntimeArgs, SecretKey, U512,
};
use once_cell::sync::Lazy;
use serde_json::json;

use crate::wasm_utils;

const OOM_INIT: (u32, Option<u32>) = (2805655325, None);
const FAILURE_ONE_ABOVE_LIMIT: (u32, Option<u32>) = (DEFAULT_MAX_TABLE_SIZE + 1, None);
const FAILURE_MAX_ABOVE_LIMIT: (u32, Option<u32>) = (DEFAULT_MAX_TABLE_SIZE, Some(u32::MAX));
const FAILURE_INIT_ABOVE_LIMIT: (u32, Option<u32>) =
    (DEFAULT_MAX_TABLE_SIZE, Some(DEFAULT_MAX_TABLE_SIZE + 1));
const ALLOWED_NO_MAX: (u32, Option<u32>) = (DEFAULT_MAX_TABLE_SIZE, None);
const ALLOWED_LIMITS: (u32, Option<u32>) = (DEFAULT_MAX_TABLE_SIZE, Some(DEFAULT_MAX_TABLE_SIZE));
// Anything larger than that fails wasmi interpreter with a runtime stack overflow.
const FAILING_BR_TABLE_SIZE: usize = DEFAULT_BR_TABLE_MAX_SIZE as usize + 1;
const FAILING_GLOBALS_SIZE: usize = DEFAULT_MAX_PARAMETER_COUNT as usize + 1;
const FAILING_PARAMS_COUNT: usize = DEFAULT_MAX_PARAMETER_COUNT as usize + 1;

static BID_ACCOUNT_1_PK: Lazy<PublicKey> = Lazy::new(|| {
    let secret_key = SecretKey::ed25519_from_bytes([204; SecretKey::ED25519_LENGTH]).unwrap();
    PublicKey::from(&secret_key)
});
static BID_ACCOUNT_1_ADDR: Lazy<AccountHash> = Lazy::new(|| AccountHash::from(&*BID_ACCOUNT_1_PK));
const BID_ACCOUNT_1_BALANCE: u64 = MINIMUM_ACCOUNT_CREATION_BALANCE;
const ADD_BID_AMOUNT_1: u64 = 95_000;
#[ignore]
#[test]
fn for_jh() {
    let mut builder = LmdbWasmTestBuilder::default();
    builder.run_genesis(LOCAL_GENESIS_REQUEST.clone());
    let exec_request_1 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "transfer_to_public_key.wasm",
        runtime_args! { "target" => BID_ACCOUNT_1_PK.clone(), "amount" => U512::from(BID_ACCOUNT_1_BALANCE) },
    )
    .build();

    builder.exec(exec_request_1).expect_success().commit();

    // let accounts = {
    //     let mut tmp: Vec<GenesisAccount> = DEFAULT_ACCOUNTS.clone();
    //     let account_1 = GenesisAccount::account(
    //         BID_ACCOUNT_1_PK.clone(),
    //         Motes::new(BID_ACCOUNT_1_BALANCE),
    //         None,
    //     );
    //     tmp.push(account_1);
    //     tmp
    // };

    // let run_genesis_request = utils::create_run_genesis_request(accounts);

    let exec_request_1 = ExecuteRequestBuilder::standard(
        *BID_ACCOUNT_1_ADDR,
        "add_bid.wasm",
        runtime_args! {
            ARG_PUBLIC_KEY => BID_ACCOUNT_1_PK.clone(),
            ARG_AMOUNT => U512::from(ADD_BID_AMOUNT_1),
            ARG_DELEGATION_RATE => 10u8,
        },
    )
    .build();
    builder.exec(exec_request_1).expect_success().commit();

    let initial_size_exceeded = vec![OOM_INIT, FAILURE_ONE_ABOVE_LIMIT];

    let max_size_exceeded = vec![FAILURE_MAX_ABOVE_LIMIT, FAILURE_INIT_ABOVE_LIMIT];

    let json_args = r#"[
        [
          "action",
          {
            "bytes": "0800000064656c6567617465",
            "parsed": "delegate",
            "cl_type": "String"
          }
        ],
        [
          "delegator",
          {
            "bytes": "02039a1e12c68e360dba69c27e4d8d766478fc284d1c0a031c66ed6d44a8bececddd",
            "parsed": "02039a1e12c68e360dba69c27e4d8d766478fc284d1c0a031c66ed6d44a8bececddd",
            "cl_type": "PublicKey"
          }
        ],
        [
          "validator",
          {
            "bytes": "017fec504c642f2b321b8591f1c3008348c57a81acafceb5a392cf8416a5fb4a3c",
            "parsed": "017fec504c642f2b321b8591f1c3008348c57a81acafceb5a392cf8416a5fb4a3c",
            "cl_type": "PublicKey"
          }
        ],
        [
          "amount",
          {
            "bytes": "050070c9b28b",
            "parsed": "600000000000",
            "cl_type": "U512"
          }
        ],
        [
          "deploy_type",
          {
            "bytes": "0b0000005374616b696e674c6f636b",
            "parsed": "StakingLock",
            "cl_type": "String"
          }
        ],
        [
          "staking_type",
          {
            "bytes": "040000004c4f434b",
            "parsed": "LOCK",
            "cl_type": "String"
          }
        ],
        [
          "from_address",
          {
            "bytes": "440000003032303339613165313263363865333630646261363963323765346438643736363437386663323834643163306130333163363665643664343461386265636563646464",
            "parsed": "02039a1e12c68e360dba69c27e4d8d766478fc284d1c0a031c66ed6d44a8bececddd",
            "cl_type": "String"
          }
        ]
    ]"#;
    let json_args: serde_json::Value = serde_json::from_str(json_args).unwrap();

    let mut args: RuntimeArgs = serde_json::from_value(json_args).unwrap();

    let from_address_bytes = base16::encode_lower(&DEFAULT_ACCOUNT_PUBLIC_KEY.to_bytes().unwrap());
    dbg!(&from_address_bytes);

    dbg!(DEFAULT_ACCOUNT_ADDR.clone());
    args.insert("delegator", DEFAULT_ACCOUNT_PUBLIC_KEY.clone())
        .unwrap();
    args.insert("validator", BID_ACCOUNT_1_PK.clone()).unwrap();
    args.insert("from_address", from_address_bytes).unwrap();

    // for (initial, maximum) in initial_size_exceeded {
    let module_bytes = fs::read("/tmp/jh.wasm").unwrap();
    let exec_request = {
        let account_hash = *DEFAULT_ACCOUNT_ADDR;
        let module_bytes: Bytes = module_bytes.into();
        let deploy_item = DeployItemBuilder::new()
            .with_address(account_hash)
            .with_session_bytes(module_bytes, args)
            .with_standard_payment(runtime_args! {
                ARG_AMOUNT => U512::from(5_100_000_000u64),
            })
            .with_authorization_keys(&[account_hash])
            .build();
        ExecuteRequestBuilder::from_deploy_item(&deploy_item)
    }
    .build();

    builder.exec(exec_request).expect_success().commit();

    dbg!(builder.last_exec_gas_consumed());

    // let error = builder.get_error().unwrap();

    // dbg!(builder.get_error());
    // }

    // for (initial, maximum) in max_size_exceeded {
    //     let module_bytes = make_oom_payload(initial, maximum);
    //     let exec_request = ExecuteRequestBuilder::module_bytes(
    //         *DEFAULT_ACCOUNT_ADDR,
    //         module_bytes,
    //         RuntimeArgs::default(),
    //     )
    //     .build();

    //     builder.exec(exec_request).expect_failure().commit();

    //     let error = builder.get_error().unwrap();

    //     assert!(
    //         matches!(
    //             error,
    //             
    // engine_state::Error::WasmPreprocessing(PreprocessingError::WasmValidation(WasmValidationError::MaxTableSizeExceeded
    // { max, actual }))             if max == DEFAULT_MAX_TABLE_SIZE && Some(actual) == maximum
    //         ),
    //         "{initial} {maximum:?} {:?}",
    //         error
    //     );
    // }
}
