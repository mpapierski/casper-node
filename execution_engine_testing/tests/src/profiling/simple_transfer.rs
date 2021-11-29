//! This executable is designed to be used to profile a single execution of a simple contract which
//! transfers an amount between two accounts.
//!
//! In order to set up the required global state for the transfer, the `state-initializer` should
//! have been run first.
//!
//! By avoiding setting up global state as part of this executable, it will allow profiling to be
//! done only on meaningful code, rather than including test setup effort in the profile results.

use std::{
    convert::TryFrom,
    env,
    fs::{self, File},
    io,
    path::PathBuf, collections::{BTreeMap, BTreeSet},
};

use clap::{crate_version, App, Arg};

use casper_engine_test_support::{
    internal::{DeployItemBuilder, ExecuteRequestBuilder, LmdbWasmTestBuilder, DEFAULT_PAYMENT, DEFAULT_RUN_GENESIS_REQUEST, InMemoryWasmTestBuilder},
    DEFAULT_ACCOUNT_ADDR,
};
use casper_execution_engine::{core::engine_state::EngineConfig, storage::{global_state::StateProvider, trie::{Trie, Pointer}}};
use casper_hashing::Digest;
use casper_types::{runtime_args, DeployHash, RuntimeArgs, U512, Key, StoredValue, CLType, KeyTag, Tagged};

use casper_engine_tests::profiling;
use rand::{thread_rng, Rng};

const ABOUT: &str = "Executes a simple contract which transfers an amount between two accounts.  \
     Note that the 'state-initializer' executable should be run first to set up the required \
     global state.";

const ROOT_HASH_ARG_NAME: &str = "root-hash";
const ROOT_HASH_ARG_VALUE_NAME: &str = "HEX-ENCODED HASH";
const ROOT_HASH_ARG_HELP: &str =
    "Initial root hash; the output of running the 'state-initializer' executable";

const VERBOSE_ARG_NAME: &str = "verbose";
const VERBOSE_ARG_SHORT: &str = "v";
const VERBOSE_ARG_LONG: &str = "verbose";
const VERBOSE_ARG_HELP: &str = "Display the transforms resulting from the contract execution";

const TRANSFER_AMOUNT: u64 = 1;

fn root_hash_arg() -> Arg<'static, 'static> {
    Arg::with_name(ROOT_HASH_ARG_NAME)
        .value_name(ROOT_HASH_ARG_VALUE_NAME)
        .help(ROOT_HASH_ARG_HELP)
}

fn verbose_arg() -> Arg<'static, 'static> {
    Arg::with_name(VERBOSE_ARG_NAME)
        .short(VERBOSE_ARG_SHORT)
        .long(VERBOSE_ARG_LONG)
        .help(VERBOSE_ARG_HELP)
}

#[derive(Debug)]
struct Args {
    root_hash: Option<Vec<u8>>,
    data_dir: PathBuf,
    verbose: bool,
}

// fn trie_stats(trie: &Trie<Key, StoredValue>) -> (usize, usize, usize) {
    
// }

#[derive(Eq, PartialEq, Ord, PartialOrd, Copy, Clone, Debug)]
enum StoredValueTag {
    CLValue,
    Account,
    ContractWasm,
    Contract,
    ContractPackage,
    Transfer,
    DeployInfo,
    EraInfo,
    Bid,
    Withdraw
}

fn print_stats(post_state_hash: &Digest, tries: &BTreeMap<Digest, Trie<Key, StoredValue>>) {
    let mut leaf_count = 0usize;
    let mut pointer_block_count = 0usize;
    let mut pointer_block_pointers = 0usize;
    let mut pointer_block_pointers_count = 0usize;
    let mut extension_count = 0usize;
    let mut cl_types: BTreeMap<CLType, usize> = BTreeMap::default();
    let mut stored_values: BTreeMap<StoredValueTag, usize> = BTreeMap::default();
    let mut keys: BTreeMap<KeyTag, usize> = BTreeMap::default();
    let mut affix_lengths: BTreeSet<usize> = BTreeSet::default();
    let mut pointer_leaf_count = 0usize;
    let mut pointer_node_count = 0usize;
    let mut unique_pointer_leafs: BTreeSet<&Digest> = BTreeSet::default();
    let mut unique_pointer_nodes: BTreeSet<&Digest> = BTreeSet::default();
    // let mut unreachable_node_pointers: BTreeSet<&Digest> = BTreeSet::default();
    // let mut unreachable_node_pointers: BTreeSet<&Digest> = BTreeSet::default();

    println!("digraph trie_store {{");
    println!(r#""{}" [fillcolor=red, style=filled]"#, base16::encode_lower(&post_state_hash.value()));

    for (state_root_hash, trie) in tries {
        match trie {
            Trie::Leaf { key, value } => {
                println!(r#""{}" [fillcolor=green, style=filled]"#, base16::encode_lower(&state_root_hash.value()));
            }
            Trie::Node { .. } | Trie::Extension { .. } => {},
        }
    }



    for (rank, (state_root_hash, trie)) in tries.iter().enumerate() {
        match trie {
            Trie::Leaf { key, value } => {
                let key_tag = key.tag();
                *keys.entry(key_tag).or_default() += 1;

                leaf_count += 1;
                let stored_value_tag = match value {

                    StoredValue::CLValue(cl_value) => {
                        *cl_types.entry(cl_value.cl_type().clone()).or_default() += 1;
                        let tag = StoredValueTag::CLValue;
                        tag
                    },
                    StoredValue::Account(_) => {
                        StoredValueTag::Account
                    },
                    StoredValue::ContractWasm(_) => {
                        StoredValueTag::ContractWasm
                    },
                    StoredValue::Contract(_) => {
                        StoredValueTag::Contract
                    },
                    StoredValue::ContractPackage(_) => {
                        StoredValueTag::ContractPackage
                    },
                    StoredValue::Transfer(_) => {
                        StoredValueTag::Transfer
                    },
                    StoredValue::DeployInfo(_) => {
                        StoredValueTag::DeployInfo
                    },
                    StoredValue::EraInfo(_) => {
                        StoredValueTag::EraInfo
                    },
                    StoredValue::Bid(_) => {
                        StoredValueTag::Bid
                    },
                    StoredValue::Withdraw(_) => {
                        StoredValueTag::Withdraw
                    },
                };
                *stored_values.entry(stored_value_tag).or_default() += 1;

                println!(r#""{from}" -> {to:?} [label="{label}"]"#,
from=base16::encode_lower(&state_root_hash.value()),
  to=stored_value_tag,
  label="Leaf",
);
                // println!(r#"{hash} -> ")
            },
            Trie::Node { pointer_block } => {
                pointer_block_count += 1;
                pointer_block_pointers += pointer_block.child_count();
                pointer_block_pointers_count += 256; // RADIX

                for (index, pointer) in pointer_block.as_indexed_pointers() {
                    match pointer {
                        Pointer::LeafPointer(digest) => {
                            pointer_leaf_count += 1;
                            unique_pointer_leafs.insert(digest);
                            println!(r#""{from}" -> "{to}" [label="{label}"; headlabel="{headlabel}"; taillabel="{taillabel}"; ]"#,
                        from=base16::encode_lower(&state_root_hash.value()),
                        to=base16::encode_lower(&digest.value()),
                        headlabel=format!("{}", index),
                        taillabel="LeafPointer",
                        label="Node",
                      );
                    
                        }
                        Pointer::NodePointer(digest) => {
                            pointer_node_count += 1;
                            unique_pointer_nodes.insert(digest);
                            println!(r#""{from}" -> "{to}" [label="{label}"; headlabel="{headlabel}"; taillabel="{taillabel}"; ]"#,
                                from=base16::encode_lower(&state_root_hash.value()),
                                to=base16::encode_lower(&digest.value()),
                                headlabel=format!("{}", index),
                                taillabel="NodePointer",
                                label="Node",
                              );
                        }
                    }   
                }

                // println!(r#""{from}" -> "{to}" [
                //     label="{label}"#,
                //     from=base16::encode_lower(&state_root_hash.value()),
                //     to=format!("{:?}", stored_value_tag),
                //     label="Leaf",
                // );
            },
            Trie::Extension { affix, pointer } => {
                affix_lengths.insert(affix.len());

                match pointer {
                    Pointer::LeafPointer(digest) => {
                        pointer_leaf_count += 1;
                        unique_pointer_leafs.insert(digest);

                        println!(r#""{from}" -> "{to}" [label="{label}"; headlabel="{headlabel}"; taillabel="{taillabel}"; ]"#,
                            from=base16::encode_lower(&state_root_hash.value()),
                            to=base16::encode_lower(&digest.value()),
                            headlabel=format!("affix={}", base16::encode_lower(affix.as_ref())),
                            taillabel="LeafPointer",
                            label="Extension",
                          );


                    }
                    Pointer::NodePointer(digest) => {
                        pointer_node_count += 1;
                        unique_pointer_nodes.insert(digest);
                        
                        println!(r#""{from}" -> "{to}" [label="{label}"; headlabel="{headlabel}"; taillabel="{taillabel}"; ]"#,
                            from=base16::encode_lower(&state_root_hash.value()),
                            to=base16::encode_lower(&digest.value()),
                            headlabel=format!("affix={}", base16::encode_lower(affix.as_ref())),
                            taillabel="NodePointer",
                            label="Extension",
                          );

                    }
                }

                extension_count += 1;
            },
        }
    }

    let unreachable_node_pointers: BTreeSet<&&Digest> = unique_pointer_nodes.iter().filter(|&&digest| !tries.contains_key(digest)).collect();
    let unreachable_leaf_pointers: BTreeSet<&&Digest> = unique_pointer_leafs.iter().filter(|&&digest| !tries.contains_key(digest)).collect();

    println!("}}");

    println!("// leaf_count: {}", leaf_count);
    println!("// pointer_block_count: {}", pointer_block_count);
    println!("// pointer_block_pointers: {}", pointer_block_pointers);
    println!("// pointer_block_pointers_count: {}", pointer_block_pointers_count);
    println!("// pointer_block_fill_ratio: {}", pointer_block_pointers as f64 / pointer_block_pointers_count as f64);
    println!("// extension_count: {}", extension_count);
    println!("// cl_types: {:?}", cl_types);
    println!("// stored_values: {:?}", stored_values);
    println!("// keys: {:?}", keys);
    println!("// affix lengths: {:?}", affix_lengths);
    println!("// pointer_leaf_count: {}", pointer_leaf_count);
    println!("// pointer_node_count: {}", pointer_node_count);
    println!("// unique_pointer_leafs: {}", unique_pointer_leafs.len());
    println!("// unique_pointer_nodes: {}", unique_pointer_nodes.len());
    println!("// unreachable_node_pointers: {}", unreachable_node_pointers.len());
    println!("// unreachable_leaf_pointers: {}", unreachable_leaf_pointers.len());

}

impl Args {
    fn new() -> Self {
        let exe_name = profiling::exe_name();
        let data_dir_arg = profiling::data_dir_arg();
        let arg_matches = App::new(&exe_name)
            .version(crate_version!())
            .about(ABOUT)
            .arg(root_hash_arg())
            .arg(data_dir_arg)
            .arg(verbose_arg())
            .get_matches();
        let root_hash = arg_matches
            .value_of(ROOT_HASH_ARG_NAME)
            .map(profiling::parse_hash);
        let data_dir = profiling::data_dir(&arg_matches);
        let verbose = arg_matches.is_present(VERBOSE_ARG_NAME);
        Args {
            root_hash,
            data_dir,
            verbose,
        }
    }
}

const STATE_HASH_FILE: &str = "state_hash.raw";
const STATE_INITIALIZER_CONTRACT: &str = "gh_2346_regression.wasm";
const NUMBER: u64 = 50_000;

fn main() {
    let args = Args::new();

    // If the required initial root hash wasn't passed as a command line arg, expect to read it in
    // from stdin to allow for it to be piped from the output of 'state-initializer'.
    let state_root_hash = {
        let hash_bytes = match args.root_hash {
            Some(root_hash) => root_hash,
            None => fs::read(STATE_HASH_FILE).unwrap(),
        };

        Digest::try_from(hash_bytes.as_slice()).unwrap()
    };


    let exec_request_2 = {
        let deploy = DeployItemBuilder::new()
            .with_address(*DEFAULT_ACCOUNT_ADDR)
            .with_deploy_hash(thread_rng().gen())
            .with_stored_session_named_key(
                "contract_hash",
                "create_domains",
                runtime_args! {
                    "number" => NUMBER,
                },
            )
            // .with_session_code(
            //     "simple_transfer.wasm",
            //     runtime_args! { "target" =>account_2_account_hash, "amount" =>
            // U512::from(TRANSFER_AMOUNT) }, )
            .with_empty_payment_bytes(runtime_args! { "amount" => (*DEFAULT_PAYMENT * 10)})
            .with_authorization_keys(&[*DEFAULT_ACCOUNT_ADDR])
            .build();

        ExecuteRequestBuilder::new().push_deploy(deploy).build()
    };

    let exec_request_3 = {
        let deploy = DeployItemBuilder::new()
            .with_address(*DEFAULT_ACCOUNT_ADDR)
            .with_deploy_hash(thread_rng().gen())
            .with_stored_session_named_key(
                "contract_hash",
                "create_domains",
                runtime_args! {
                    "number" => NUMBER,
                },
            )
            // .with_session_code(
            //     "simple_transfer.wasm",
            //     runtime_args! { "target" =>account_2_account_hash, "amount" =>
            // U512::from(TRANSFER_AMOUNT) }, )
            .with_empty_payment_bytes(runtime_args! { "amount" => (*DEFAULT_PAYMENT * 10)})
            .with_authorization_keys(&[*DEFAULT_ACCOUNT_ADDR])
            .build();

        ExecuteRequestBuilder::new().push_deploy(deploy).build()
    };

    let engine_config = EngineConfig::default();

    // let mut test_builder =
        // LmdbWasmTestBuilder::open(&args.data_dir, engine_config, state_root_hash);
    let mut test_builder = InMemoryWasmTestBuilder::default();
    test_builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    
    // let account_1_account_hash = profiling::account_1_account_hash();
    // let account_2_account_hash = profiling::account_2_account_hash();
    let install_request = {
        let deploy = DeployItemBuilder::new()
            .with_address(*DEFAULT_ACCOUNT_ADDR)
            .with_deploy_hash([1; 32])
            .with_session_code(STATE_INITIALIZER_CONTRACT, RuntimeArgs::default())
            .with_empty_payment_bytes(runtime_args! { "amount" => *DEFAULT_PAYMENT, })
            .with_authorization_keys(&[*DEFAULT_ACCOUNT_ADDR])
            .build();

        ExecuteRequestBuilder::new().push_deploy(deploy).build()
    };

    eprintln!("before install");

    test_builder.exec(install_request).expect_success().commit();
    // let state =

    let tries_after_stored_contract =  test_builder.get_engine_state().state().get_tries();
    // eprintln!()

    eprintln!("before exec");
    test_builder.exec(exec_request_2).expect_success().commit();

    if args.verbose {
        println!("{:#?}", test_builder.get_transforms());
    }

    let post_state_hash = test_builder.get_post_state_hash();
    fs::write(STATE_HASH_FILE, &post_state_hash).unwrap();
    let state = test_builder.get_engine_state();
    let total_size = state.state().total_size();
    println!("// total {}", total_size);


    let tries = state.state().get_tries();
    println!("// total tries: {}", tries.len() - tries_after_stored_contract.len());

    let mut diff = tries.clone();

    // for digest in tries_after_stored_contract.keys() {

    // }
    print_stats(&post_state_hash,  &diff);

    // test_builder.exec(exec_request_3).expect_success().commit();

    // let state = test_builder.get_engine_state();
    // let total_size = state.total_size();
    // println!("2. total {}", total_size);
    
}
