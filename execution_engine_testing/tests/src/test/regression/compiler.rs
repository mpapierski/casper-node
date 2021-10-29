use std::time::Instant;

use num_traits::Zero;

use casper_engine_test_support::{
    internal::{
        DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNTS,
        DEFAULT_RUN_GENESIS_REQUEST,
    },
    AccountHash, DEFAULT_ACCOUNT_ADDR,
};
use casper_execution_engine::{
    core::{
        engine_state::{self, Error as CoreError, MAX_PAYMENT},
        execution,
    },
    shared::{opcode_costs::DEFAULT_NOP_COST, wasm},
};
use casper_types::{
    bytesrepr::Bytes, contracts::DEFAULT_ENTRY_POINT_NAME, runtime_args, system::mint, Gas, Key,
    RuntimeArgs, URef, U256, U512,
};
use parity_wasm::{
    builder,
    elements::{Instruction, Instructions},
};

const ARG_AMOUNT: &str = "amount";
const CONTRACT_TRANSFER_TO_EXISTING_ACCOUNT: &str = "transfer_to_existing_account.wasm";

#[ignore]
#[test]
fn should_run_endless_loop() {
    let exec = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "endless_loop.wasm",
        RuntimeArgs::default(),
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    let start = Instant::now();
    builder.exec(exec).commit();
    let stop = start.elapsed();

    let maybe_error = builder.get_error();
    assert!(
        matches!(
            maybe_error,
            Some(engine_state::Error::Exec(execution::Error::GasLimit))
        ),
        "{:?}",
        maybe_error
    );
}

#[ignore]
#[test]
fn should_try_to_exercise_cache() {
    let exec1 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "endless_loop.wasm",
        RuntimeArgs::default(),
    )
    .build();
    let exec2 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "endless_loop.wasm",
        RuntimeArgs::default(),
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let start1 = Instant::now();
    builder.exec(exec1).commit();
    let stop1 = start1.elapsed();
    eprintln!("elapsed1 {:?}", stop1);

    let maybe_error = builder.get_error();
    assert!(
        matches!(
            maybe_error,
            Some(engine_state::Error::Exec(execution::Error::GasLimit))
        ),
        "{:?}",
        maybe_error
    );

    let start2 = Instant::now();
    builder.exec(exec2).commit();
    let stop2 = start2.elapsed();

    let maybe_error = builder.get_error();
    assert!(
        matches!(
            maybe_error,
            Some(engine_state::Error::Exec(execution::Error::GasLimit))
        ),
        "{:?}",
        maybe_error
    );
    eprintln!("elapsed2 {:?}", stop2);
}
const ARG_ACCOUNTS: &str = "accounts";
const ARG_SEED_AMOUNT: &str = "seed_amount";

#[ignore]
#[test]
fn should_run_create_200_accounts() {
    const AMOUNT: u32 = 200;
    let seed_amount = U512::one();

    let accounts: Vec<AccountHash> = (1u32..=AMOUNT)
        .map(|val| {
            let val = U256::from(val);
            let mut bytes = [0u8; 32];
            val.to_big_endian(&mut bytes);
            AccountHash::new(bytes)
        })
        .collect();
    assert_eq!(accounts.len(), AMOUNT as usize);

    let exec = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "create_accounts.wasm",
        runtime_args! {
            ARG_ACCOUNTS => accounts.clone(),
            ARG_SEED_AMOUNT => seed_amount,
        },
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    let start = Instant::now();
    builder.exec(exec).expect_success().commit();
    let stop = start.elapsed();

    for account in accounts {
        let account = builder.get_account(account).unwrap();
        let main_purse = account.main_purse();
        let balance = builder.get_purse_balance(main_purse);
        assert_eq!(balance, seed_amount);
    }
}

const ARG_TOTAL_PURSES: &str = "total_purses";

#[ignore]
#[test]
fn should_run_create_200_purses() {
    const AMOUNT: u64 = 200;
    let seed_amount = U512::one();

    let exec = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "create_purses.wasm",
        runtime_args! { ARG_TOTAL_PURSES => AMOUNT, ARG_SEED_AMOUNT => seed_amount },
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    builder.exec(exec).expect_success().commit();

    let purses: Vec<URef> = {
        let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

        let mut purses = Vec::new();

        // let main_purse = account.main_purse();

        for (name, key) in account.named_keys() {
            if name.starts_with("purse:") {
                let uref = key.into_uref().unwrap();
                let balance = builder.get_purse_balance(uref);
                assert_eq!(balance, seed_amount);
                purses.push(uref);
            }
        }

        purses
    };
    assert_eq!(purses.len(), AMOUNT as usize);
}

const ACCOUNT_1_ADDR: AccountHash = AccountHash::new([161u8; 32]);

#[ignore]
#[test]
fn simple_transfer() {
    let create_account_request = {
        let transfer_amount = U512::from(1_000_000);

        let id: Option<u64> = None;
        let transfer_args = runtime_args! {
            mint::ARG_TARGET => ACCOUNT_1_ADDR,
            mint::ARG_AMOUNT => transfer_amount,
            mint::ARG_ID => id,
        };

        ExecuteRequestBuilder::transfer(*DEFAULT_ACCOUNT_ADDR, transfer_args).build()
    };
    let exec1 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_TRANSFER_TO_EXISTING_ACCOUNT,
        runtime_args! {
            "target" => ACCOUNT_1_ADDR,
            "amount" => U512::from(1_000_000),
        },
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);
    builder
        .exec(create_account_request)
        .expect_success()
        .commit();

    let start1 = Instant::now();
    builder.exec(exec1).expect_success().commit();
    let stop1 = start1.elapsed();
}
