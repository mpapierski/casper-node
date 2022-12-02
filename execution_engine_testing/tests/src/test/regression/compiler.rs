use std::time::Instant;

use casper_engine_test_support::{
    instrumented, DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder,
    DEFAULT_ACCOUNT_ADDR, PRODUCTION_RUN_GENESIS_REQUEST,
};
use num_traits::Zero;

use casper_execution_engine::{
    core::{
        engine_state::{self, Error as CoreError, MAX_PAYMENT},
        execution,
    },
    shared::opcode_costs::DEFAULT_NOP_COST,
};
use casper_types::{
    account::AccountHash, bytesrepr::Bytes, contracts::DEFAULT_ENTRY_POINT_NAME, runtime_args,
    system::mint, Gas, Key, RuntimeArgs, URef, U256, U512,
};
use parity_wasm::{
    builder,
    elements::{Instruction, Instructions},
};
use wabt::wat2wasm;

const ARG_AMOUNT: &str = "amount";
const CONTRACT_TRANSFER_TO_EXISTING_ACCOUNT: &str = "transfer_to_existing_account.wasm";

const SLOW_INPUT: &str = r#"(module
    (type $CASPER_RET_TY (func (param i32 i32)))
    (type $CALL_TY (func))
    (type $BUSY_LOOP_TY (func (param i32 i32 i32) (result i32)))

    (func $CALL_FN (type $CALL_TY)
      (local i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      local.get 0
      i64.const -2259106222686656124
      local.get 0
      i32.const 16
      i32.add
      i32.const 18
      i32.const 50000
      call $BUSY_LOOP_FN
      drop
      local.get 0
      i32.const 12
      i32.add
      i32.const 770900
      call 0
      unreachable)
    (func $BUSY_LOOP_FN (type $BUSY_LOOP_TY) (param i32 i32 i32) (result i32)
      (local i32)
      loop  ;; label = @1
        i32.const 0
        i32.eqz
        br_if 0 (;@1;)
        local.get 0
        local.set 3
        loop  ;; label = @2
          local.get 3
          local.get 1
          i32.store8
          local.get 3
          local.set 3
          local.get 2
          i32.const -1
          i32.add
          local.tee 2
          br_if 0 (;@2;)
        end
      end
      local.get 0)
    (memory $MEM 11)
    (export "memory" (memory 0))
    (export "call" (func $CALL_FN)))"#;

#[ignore]
#[test]
fn should_execute_wasm_without_imports() {
    let minimum_deploy_payment = U512::from(DEFAULT_NOP_COST);

    let do_minimum_request = {
        let account_hash = *DEFAULT_ACCOUNT_ADDR;
        let session_args = RuntimeArgs::default();
        let deploy_hash = [42; 32];

        ExecuteRequestBuilder::module_bytes(
            *DEFAULT_ACCOUNT_ADDR,
            super::regression_20210924::do_minimum_bytes(),
            session_args,
        )
        .build()
    };

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    builder
        .exec_instrumented(instrumented!(do_minimum_request))
        .expect_success()
        .commit();
    builder.expect_success().commit();
}
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
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    let start = Instant::now();
    builder.exec_instrumented(instrumented!(exec));
    let end = start.elapsed();
    eprintln!("elapsed {:?}", end);
    builder.commit();

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

// #[ignore]
// #[test]
// fn should_run_slow_input() {
//     let slow_input = wat2wasm(SLOW_INPUT).expect("should compile");

//     let exec = ExecuteRequestBuilder::module_bytes(
//         *DEFAULT_ACCOUNT_ADDR,
//         slow_input,
//         RuntimeArgs::default(),
//     )
//     .build();

//     let mut builder = InMemoryWasmTestBuilder::default();
//     builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);

//     let start = Instant::now();
//     builder.exec_instrumented(instrumented!(exec));
//     let end = start.elapsed();
//     eprintln!("elapsed {:?}", end);

//     let maybe_error = builder.get_error();
//     assert!(
//         matches!(
//             maybe_error,
//             Some(engine_state::Error::Exec(execution::Error::GasLimit))
//         ),
//         "{:?}",
//         maybe_error
//     );
// }

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
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);

    let start1 = Instant::now();
    builder.exec_instrumented(instrumented!(exec1)).commit();
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
    builder.exec_instrumented(instrumented!(exec2)).commit();
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
    let expected_balance = U512::one();
    let seed_amount = U512::from(AMOUNT) * expected_balance;

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
            ARG_AMOUNT => seed_amount,
        },
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    let start = Instant::now();
    builder
        .exec_instrumented(instrumented!(exec))
        .expect_success()
        .commit();
    let stop = start.elapsed();

    for account in accounts {
        let account = builder.get_account(account).unwrap();
        let main_purse = account.main_purse();
        let balance = builder.get_purse_balance(main_purse);
        assert_eq!(balance, expected_balance);
    }
}

const ARG_TOTAL_PURSES: &str = "total_purses";

#[ignore]
#[test]
fn should_run_create_200_purses() {
    const AMOUNT: u64 = 200;
    let purse_amount = U512::one();
    let seed_amount = U512::from(AMOUNT) * purse_amount;

    let exec = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "create_purses.wasm",
        runtime_args! { ARG_TOTAL_PURSES => AMOUNT, ARG_AMOUNT => seed_amount },
    )
    .build();

    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    builder
        .exec_instrumented(instrumented!(exec))
        .expect_success()
        .commit();

    let purses: Vec<URef> = {
        let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

        let mut purses = Vec::new();

        // let main_purse = account.main_purse();

        for (name, key) in account.named_keys() {
            if name.starts_with("purse:") {
                let uref = key.into_uref().unwrap();
                let balance = builder.get_purse_balance(uref);
                assert_eq!(balance, purse_amount);
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
    builder.run_genesis(&PRODUCTION_RUN_GENESIS_REQUEST);
    builder
        .exec_instrumented(instrumented!(create_account_request))
        .expect_success()
        .commit();

    let start1 = Instant::now();
    builder
        .exec_instrumented(instrumented!(exec1))
        .expect_success()
        .commit();
    let stop1 = start1.elapsed();
}
