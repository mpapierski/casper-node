use casper_engine_test_support::{AccountHash, DEFAULT_ACCOUNT_ADDR, internal::{DEFAULT_RUN_GENESIS_REQUEST, ExecuteRequestBuilder, InMemoryWasmTestBuilder, StepRequestBuilder, UpgradeRequestBuilder}};
use casper_types::{EraId, ProtocolVersion, RuntimeArgs, U256, U512, account::AccountHashBytes, runtime_args, system::mint};

use crate::lmdb_fixture;

const DEFAULT_ACTIVATION_POINT: EraId = EraId::new(1);

#[ignore]
#[test]
fn should_run_gh_1767() {
    // This test runs a contract that's after every call extends the same key with
    // more data

    let (mut builder, lmdb_fixture_state, _temp_dir) = lmdb_fixture::builder_from_global_state_fixture(lmdb_fixture::RELEASE_1_3_1);

    let current_protocol_version = lmdb_fixture_state.genesis_protocol_version();

    let old_protocol_data = builder
        .get_engine_state()
        .get_protocol_data(current_protocol_version)
        .expect("should have result")
        .expect("should have protocol data");

    let new_protocol_version = ProtocolVersion::from_parts(
        current_protocol_version.value().major,
        current_protocol_version.value().minor + 1,
        0,
    );

    let mut upgrade_request = {
        UpgradeRequestBuilder::new()
            .with_current_protocol_version(current_protocol_version)
            .with_new_protocol_version(new_protocol_version)
            .with_activation_point(DEFAULT_ACTIVATION_POINT)
            .build()
    };

    builder
        .upgrade_with_upgrade_request(&mut upgrade_request)
        .expect_upgrade_success();

    //   1. Send batch of Wasm deploys
        const ARG_TARGET: &str = "target";
        const ARG_AMOUNT: &str = "amount";

        let protocol_version = ProtocolVersion::from_parts(1, 3, 1);

        for account_hash_bytes in (1..100).map(U256::from) {
            let mut account_hash_raw = AccountHashBytes::default();
            account_hash_bytes.to_big_endian(&mut account_hash_raw);
            let account_hash = AccountHash::new(account_hash_raw);
            let amount = U512::one();

            let exec_request = ExecuteRequestBuilder::standard(
                *DEFAULT_ACCOUNT_ADDR,
                "transfer_to_account_u512.wasm",
                runtime_args! {
                    ARG_TARGET => account_hash,
                    ARG_AMOUNT => amount,
                },
            )
            .with_protocol_version(protocol_version)
            .build();

            builder.exec(exec_request).expect_success().commit();
        }

        for account_index in (101..200).map(U256::from) {
            let mut account_hash_raw = AccountHashBytes::default();
            account_index.to_big_endian(&mut account_hash_raw);
            let account_hash = AccountHash::new(account_hash_raw);
            let amount = U512::one();

            // let exec_request = ExecuteRequestBuilder::standard(account_hash,
            // "transfer_to_account_u512.wasm", runtime_args! { ARG_TARGET => *DEFAULT_ACCOUNT_ADDR,
            // ARG_AMOUNT => amount }).build();
            let transfer_request = ExecuteRequestBuilder::transfer(
                *DEFAULT_ACCOUNT_ADDR,
                runtime_args! {
                    mint::ARG_TARGET => account_hash,
                    mint::ARG_AMOUNT => amount,
                    mint::ARG_ID => Some(account_index.as_u64()),
                },
            )
            .with_protocol_version(protocol_version)
            .build();

            builder.exec(transfer_request).expect_success().commit();
        }

        let step_request = StepRequestBuilder::new()
            .with_run_auction(true)
            .with_next_era_id(EraId::from(2))
            .with_parent_state_hash(builder.get_post_state_hash())
            .with_protocol_version(protocol_version)
            .build();

        builder.step(step_request);
    
}
