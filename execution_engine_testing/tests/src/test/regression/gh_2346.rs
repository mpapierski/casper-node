use std::{collections::BTreeMap, env, time::Instant};

use casper_engine_test_support::internal::{
    DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder, LmdbWasmTestBuilder,
    UpgradeRequestBuilder, DEFAULT_ACCOUNT_ADDR, DEFAULT_ACCOUNT_PUBLIC_KEY, DEFAULT_PAYMENT,
    DEFAULT_PROTOCOL_VERSION, DEFAULT_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::{
    core::{
        engine_state::{Error, ExecuteRequest, SystemContractRegistry},
        execution,
    },
    storage::global_state::in_memory::InMemoryGlobalState,
};
use casper_hashing::Digest;
use casper_types::{
    account::AccountHash,
    runtime_args,
    system::{auction, auction::DelegationRate, mint, standard_payment::ARG_AMOUNT},
    AccessRights, CLTyped, CLValue, ContractHash, ContractPackageHash, EraId, Key, ProtocolVersion,
    RuntimeArgs, StoredValue, StoredValueTypeMismatch, URef, U256, U512,
};
use once_cell::sync::Lazy;
use tempfile::TempDir;

use crate::lmdb_fixture::{self, LmdbFixtureState};

const GH_2346_REGRESSION: &str = "gh_2346_regression.wasm";
// const GH_2346_REGRESSION: &str = "a2c.wasm";
const ARG_NUMBER: &str = "number";
const CONTRACT_HASH_NAME: &str = "contract_hash";
// const CONTRACT_HASH_NAME: &str = "test-mgr-latest-version-contract";
const CREATE_DOMAINS_ENTRYPOINT: &str = "create_domains";

const TOTAL_DOMAINS: u64 = 50_000;

#[derive(Copy, Clone, PartialEq, Debug)]
enum StorageType {
    InMemory,
    Lmdb,
}

static GLOBAL_STATE: once_cell::sync::Lazy<StorageType> =
    Lazy::new(|| match env::var_os("STORAGE_TYPE") {
        Some(os_str) if os_str == "in_memory" => StorageType::InMemory,
        Some(os_str) if os_str == "lmdb" => StorageType::Lmdb,
        Some(os_str) => panic!("Invalid storage type {:?}", os_str),
        None => StorageType::InMemory,
    });

fn apply_global_state_update(
    builder: &LmdbWasmTestBuilder,
    post_state_hash: Digest,
) -> BTreeMap<Key, StoredValue> {
    let key = URef::new([0u8; 32], AccessRights::all()).into();

    let system_contract_hashes = builder
        .query(Some(post_state_hash), key, &Vec::new())
        .expect("Must have stored system contract hashes")
        .as_cl_value()
        .expect("must be CLValue")
        .clone()
        .into_t::<SystemContractRegistry>()
        .expect("must convert to btree map");

    let mut global_state_update = BTreeMap::<Key, StoredValue>::new();
    let registry = CLValue::from_t(system_contract_hashes)
        .expect("must convert to StoredValue")
        .into();

    global_state_update.insert(Key::SystemContractRegistry, registry);

    global_state_update
}
enum GenericTestBuilder {
    InMemory(InMemoryWasmTestBuilder),
    Lmdb {
        builder: LmdbWasmTestBuilder,
        fixture: LmdbFixtureState,
        temp_dir: TempDir,
    },
}

impl Default for GenericTestBuilder {
    fn default() -> Self {
        match *GLOBAL_STATE {
            StorageType::InMemory => {
                let mut builder = InMemoryWasmTestBuilder::default();
                builder.run_genesis(&*DEFAULT_RUN_GENESIS_REQUEST);
                GenericTestBuilder::InMemory(builder)
            }
            StorageType::Lmdb => {
                let (mut builder, fixture, temp_dir) =
                    lmdb_fixture::builder_from_global_state_fixture(lmdb_fixture::RELEASE_1_3_1);

                let global_state_update =
                    apply_global_state_update(&mut builder, fixture.post_state_hash);

                let previous_protocol_version = fixture.genesis_protocol_version();

                let current_protocol_version = fixture.genesis_protocol_version();

                let new_protocol_version =
                    ProtocolVersion::from_parts(current_protocol_version.value().major + 1, 0, 0);

                const DEFAULT_ACTIVATION_POINT: EraId = EraId::new(1);

                let mut upgrade_request = {
                    UpgradeRequestBuilder::new()
                        .with_current_protocol_version(previous_protocol_version)
                        .with_new_protocol_version(new_protocol_version)
                        .with_activation_point(DEFAULT_ACTIVATION_POINT)
                        .with_global_state_update(global_state_update)
                        .build()
                };

                builder
                    .upgrade_with_upgrade_request(
                        *builder.get_engine_state().config(),
                        &mut upgrade_request,
                    )
                    .expect_upgrade_success();

                GenericTestBuilder::Lmdb {
                    builder,
                    fixture,
                    temp_dir,
                }
            }
        }
    }
}

impl GenericTestBuilder {
    pub fn exec(&mut self, exec_request: ExecuteRequest) {
        match self {
            GenericTestBuilder::InMemory(in_memory) => {
                in_memory.exec(exec_request);
            }
            GenericTestBuilder::Lmdb { builder: lmdb, .. } => {
                lmdb.exec(exec_request);
            }
        }
    }

    // pub fn exec_against_cache(&mut self, exec_request: ExecuteRequest) {
    //     match self {
    //         GenericTestBuilder::InMemory(in_memory) => {
    //             in_memory.exec(exec_request);
    //         }
    //         GenericTestBuilder::Lmdb { builder: lmdb, .. } => {
    //             lmdb.exec_against_cache(exec_request);
    //         }
    //     }
    // }

    pub fn commit(&mut self) {
        match self {
            GenericTestBuilder::InMemory(in_memory) => {
                in_memory.commit();
            }
            GenericTestBuilder::Lmdb { builder, .. } => {
                builder.commit();
            }
        }
    }

    // pub fn commit_cache(&mut self) {
    //     match self {
    //         GenericTestBuilder::InMemory(in_memory) => {
    //             in_memory.commit();
    //         }
    //         GenericTestBuilder::Lmdb { builder, .. } => {
    //             builder.commit_cache();
    //         }
    //     }
    // }

    pub fn expect_success(&mut self) {
        match self {
            GenericTestBuilder::InMemory(in_memory) => {
                in_memory.expect_success();
            }
            GenericTestBuilder::Lmdb { builder, .. } => {
                builder.expect_success();
            }
        }
    }

    pub fn protocol_version(&mut self) -> ProtocolVersion {
        match self {
            GenericTestBuilder::InMemory(_) => *DEFAULT_PROTOCOL_VERSION,
            GenericTestBuilder::Lmdb { fixture, .. } => {
                let current_protocol_version = fixture.genesis_protocol_version();
                let new_protocol_version =
                    ProtocolVersion::from_parts(current_protocol_version.value().major + 1, 0, 0);
                new_protocol_version
            }
        }
    }

    // pub fn make_exec_builder(&self) -> ExecuteRequestBuilder {
    //     let mut builder = ExecuteRequestBuilder::default();
    //     match self {
    //         GenericTestBuilder::InMemory(_) => builder,
    //         GenericTestBuilder::Lmdb { builder, fixture, temp_dir } => {
    //             let current_protocol_version = fixture.genesis_protocol_version();

    //             let new_protocol_version =
    //                 ProtocolVersion::from_parts(current_protocol_version.value().major + 1, 0,
    // 0);

    //                 builder.with_protocol_version(new_protocol_version);

    //                 builder
    //         }
    //     }
    // }
    pub fn disk_size(&mut self) -> usize {
        match self {
            GenericTestBuilder::InMemory(builder) => 0,
            GenericTestBuilder::Lmdb {
                builder,
                fixture,
                temp_dir,
            } => 0,
        }
    }

    pub fn flush(&mut self) {
        match self {
            GenericTestBuilder::InMemory(_) => {}
            GenericTestBuilder::Lmdb { builder, .. } => builder.flush_environment(),
        }
    }
}

#[ignore]
#[test]
fn gh_2346_should_execute_without_cache() {
    let mut builder = setup();

    for i in 1.. {
        let deploy_hash = {
            let val = U256::from(i);
            let mut deploy_hash = [0; 32];
            val.to_big_endian(&mut deploy_hash);
            deploy_hash
        };
        println!("{}", base16::encode_lower(&deploy_hash));
        // let deploy_hash = U256::from(f)

        let exec_request_1 = {
            let sender = *DEFAULT_ACCOUNT_ADDR;
            // let deploy_hash = [42; 32];
            let payment_amount = *DEFAULT_PAYMENT * 10;
            let payment_args = runtime_args! {
                ARG_AMOUNT => payment_amount,
            };
            let session_args = runtime_args! {
                ARG_NUMBER => TOTAL_DOMAINS,
            };
            let deploy = DeployItemBuilder::new()
                .with_address(sender)
                .with_stored_session_named_key(
                    CONTRACT_HASH_NAME,
                    CREATE_DOMAINS_ENTRYPOINT,
                    session_args,
                )
                .with_empty_payment_bytes(payment_args)
                .with_authorization_keys(&[sender])
                .with_deploy_hash(deploy_hash)
                .build();

            ExecuteRequestBuilder::new()
                .with_protocol_version(builder.protocol_version())
                .push_deploy(deploy)
                .build()
        };

        let now = Instant::now();

        builder.exec(exec_request_1);

        let exec_dur = now.elapsed();
        println!("{} exec ms {}", i, exec_dur.as_millis());
        builder.expect_success();
        builder.commit();
        println!("{} commit", i);
        builder.flush();

        // drop(builder);

        let commit_dur = now.elapsed() - exec_dur;

        eprintln!(
            "nocache,{i},{exec},{commit}",
            i = i,
            exec = exec_dur.as_millis(),
            commit = commit_dur.as_millis()
        );
    }

    // for n in 500..TOTAL_DOMAINS {

    // }
}

// #[ignore]
// #[test]
// fn setup_only() {
//     let mut builder = setup();
// }

fn setup() -> GenericTestBuilder {
    let mut builder = GenericTestBuilder::default();

    let install_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        GH_2346_REGRESSION,
        RuntimeArgs::default(),
    )
    .with_protocol_version(builder.protocol_version())
    .build();

    builder.exec(install_request);
    builder.expect_success();
    builder.commit();

    builder
}

// #[ignore]
// #[test]
// fn gh_2346_should_execute_with_cache() {
//     let mut builder = setup();

//     for i in 1u64.. {

//         let deploy_hash = {
//             let val = U256::from(i);
//             let mut deploy_hash = [0; 32];
//             val.to_big_endian(&mut deploy_hash);
//             deploy_hash
//         };

//         let exec_request_1 = {
//             let sender = *DEFAULT_ACCOUNT_ADDR;
//             let payment_amount = *DEFAULT_PAYMENT * 10;
//             let payment_args = runtime_args! {
//                 ARG_AMOUNT => payment_amount,
//             };
//             let session_args = runtime_args! {
//                 ARG_NUMBER => TOTAL_DOMAINS,
//             };
//             let deploy = DeployItemBuilder::new()
//                 .with_address(sender)
//                 .with_stored_session_named_key(
//                     CONTRACT_HASH_NAME,
//                     CREATE_DOMAINS_ENTRYPOINT,
//                     session_args,
//                 )
//                 .with_empty_payment_bytes(payment_args)
//                 .with_authorization_keys(&[sender])
//                 .with_deploy_hash(deploy_hash)
//                 .build();

//             ExecuteRequestBuilder::new()
//                 .with_protocol_version(builder.protocol_version())
//                 .push_deploy(deploy)
//                 .build()
//         };

//         let now = Instant::now();

//         builder.exec_against_cache(exec_request_1);

//         let exec_dur = now.elapsed();
//         builder.expect_success();

//         builder.commit_cache();

//         builder.flush();

//         let commit_dur = now.elapsed() - exec_dur;

//         eprintln!("cache,{i},{exec},{commit}", i=i, exec=exec_dur.as_millis(),
// commit=commit_dur.as_millis());     }
// }
