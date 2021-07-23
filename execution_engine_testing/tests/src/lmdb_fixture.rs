use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use casper_engine_test_support::internal::LmdbWasmTestBuilder;
use casper_types::ProtocolVersion;
use fs_extra::dir;
use serde::{Deserialize, Serialize};

use casper_execution_engine::{
    core::engine_state::{run_genesis_request::RunGenesisRequest, EngineConfig, ExecuteRequest},
    shared::newtypes::Blake2bHash,
};
use tempfile::TempDir;

pub const RELEASE_1_2_0: &str = "release_1_2_0";
pub const RELEASE_1_3_1: &str = "release_1_3_1";
const STATE_JSON_FILE: &str = "state.json";
const FIXTURES_DIRECTORY: &str = "fixtures";
const GENESIS_PROTOCOL_VERSION_FIELD: &str = "protocol_version";

fn path_to_lmdb_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES_DIRECTORY)
}

/// Contains serialized genesis config.
#[derive(Serialize, Deserialize)]
pub struct LmdbFixtureState {
    /// Serializes as unstructured JSON value because [`RunGenesisRequest`] might change over time
    /// and likely old fixture might not deserialize cleanly in the future.
    pub genesis_request: serde_json::Value,
    #[serde(
        serialize_with = "hex::serialize",
        deserialize_with = "hex::deserialize"
    )]
    pub post_state_hash: Blake2bHash,
}

impl LmdbFixtureState {
    pub fn genesis_protocol_version(&self) -> ProtocolVersion {
        serde_json::from_value(
            self.genesis_request
                .get(GENESIS_PROTOCOL_VERSION_FIELD)
                .cloned()
                .unwrap(),
        )
        .expect("should have protocol version field")
    }
}

/// Creates a [`LmdbWasmTestBuilder`] from a named fixture directory.
///
/// As part of this process a new temporary directory will be created to store LMDB files from given
/// fixture, and a builder will be created using it.
///
/// This function returns a triple of the builder, a [`LmdbFixtureState`] which contains serialized
/// genesis request for given fixture, and a temporary directory which has to be kept in scope.
pub fn builder_from_global_state_fixture(
    fixture_name: &str,
) -> (LmdbWasmTestBuilder, LmdbFixtureState, TempDir) {
    let source = path_to_lmdb_fixtures().join(fixture_name);
    let to = tempfile::tempdir().expect("should create temp dir");
    fs_extra::copy_items(&[source], &to, &dir::CopyOptions::default())
        .expect("should copy global state fixture");

    let path_to_state = to.path().join(fixture_name).join(STATE_JSON_FILE);
    let lmdb_fixture_state: LmdbFixtureState =
        serde_json::from_reader(File::open(&path_to_state).unwrap()).unwrap();
    let path_to_gs = to.path().join(fixture_name);
    (
        LmdbWasmTestBuilder::open(
            &path_to_gs,
            EngineConfig::default(),
            lmdb_fixture_state.post_state_hash,
        ),
        lmdb_fixture_state,
        to,
    )
}

/// Creates a new fixture with a name.
///
/// This process is currently manual. The process to do this is to check out a release branch, call
/// this function to generate (i.e. `generate_fixture("release_1_3_0")`) and persist it in version
/// control.
pub fn generate_fixture(
    name: &str,
    genesis_request: RunGenesisRequest,
    post_genesis_setup: impl FnOnce(&mut LmdbWasmTestBuilder),
) -> Result<(), Box<dyn std::error::Error>> {
    let lmdb_fixtures_root = path_to_lmdb_fixtures();
    let fixture_root = lmdb_fixtures_root.join(name);

    let engine_config = EngineConfig::default();
    let mut builder = LmdbWasmTestBuilder::new_with_config(&fixture_root, engine_config);

    builder.run_genesis(&genesis_request);

    // You can customize the fixture post genesis with a callable.
    post_genesis_setup(&mut builder);

    let post_state_hash = builder.get_post_state_hash();

    let state = LmdbFixtureState {
        genesis_request: serde_json::to_value(genesis_request)?,
        post_state_hash,
    };
    let serialized_state = serde_json::to_string_pretty(&state)?;
    let mut f = File::create(&fixture_root.join(STATE_JSON_FILE))?;
    f.write_all(serialized_state.as_bytes())?;
    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use casper_engine_test_support::{
//         internal::{
//             ExecuteRequestBuilder, StepRequestBuilder, DEFAULT_EXEC_CONFIG,
//             DEFAULT_GENESIS_CONFIG_HASH,
//         },
//         AccountHash, DEFAULT_ACCOUNT_ADDR,
//     };
//     use casper_types::{
//         account::AccountHashBytes, runtime_args, system::mint, EraId, RuntimeArgs, U256, U512,
//     };

//     use super::*;

//     fn initialize_v1_3_1(builder: &mut LmdbWasmTestBuilder) {
//         // 1. Send batch of Wasm deploys
//         const ARG_TARGET: &str = "target";
//         const ARG_AMOUNT: &str = "amount";

//         let protocol_version = ProtocolVersion::from_parts(1, 3, 1);

//         for account_hash_bytes in (1..100).map(U256::from) {
//             let mut account_hash_raw = AccountHashBytes::default();
//             account_hash_bytes.to_big_endian(&mut account_hash_raw);
//             let account_hash = AccountHash::new(account_hash_raw);
//             let amount = U512::one();

//             let exec_request = ExecuteRequestBuilder::standard(
//                 *DEFAULT_ACCOUNT_ADDR,
//                 "transfer_to_account_u512.wasm",
//                 runtime_args! {
//                     ARG_TARGET => account_hash,
//                     ARG_AMOUNT => amount,
//                 },
//             )
//             .with_protocol_version(protocol_version)
//             .build();

//             builder.exec(exec_request).expect_success().commit();
//         }

//         for account_index in (101..200).map(U256::from) {
//             let mut account_hash_raw = AccountHashBytes::default();
//             account_index.to_big_endian(&mut account_hash_raw);
//             let account_hash = AccountHash::new(account_hash_raw);
//             let amount = U512::one();

//             // let exec_request = ExecuteRequestBuilder::standard(account_hash,
//             // "transfer_to_account_u512.wasm", runtime_args! { ARG_TARGET => *DEFAULT_ACCOUNT_ADDR,
//             // ARG_AMOUNT => amount }).build();
//             let transfer_request = ExecuteRequestBuilder::transfer(
//                 *DEFAULT_ACCOUNT_ADDR,
//                 runtime_args! {
//                     mint::ARG_TARGET => account_hash,
//                     mint::ARG_AMOUNT => amount,
//                     mint::ARG_ID => Some(account_index.as_u64()),
//                 },
//             )
//             .with_protocol_version(protocol_version)
//             .build();

//             builder.exec(transfer_request).expect_success().commit();
//         }

//         let step_request = StepRequestBuilder::new()
//             .with_run_auction(true)
//             .with_next_era_id(EraId::from(1))
//             .with_parent_state_hash(builder.get_post_state_hash())
//             .with_protocol_version(protocol_version)
//             .build();

//         builder.step(step_request);
//     }

//     #[test]
//     fn gen_131() {
//         let protocol_version = ProtocolVersion::from_parts(1, 3, 1);
//         let genesis_request = RunGenesisRequest::new(
//             *DEFAULT_GENESIS_CONFIG_HASH,
//             protocol_version,
//             DEFAULT_EXEC_CONFIG.clone(),
//         );
//         generate_fixture(RELEASE_1_3_1, genesis_request, initialize_v1_3_1).expect("ok");
//     }
// }
