use std::{collections::BTreeSet, iter::FromIterator};

use casper_engine_test_support::{
    internal::{
        ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNT_PUBLIC_KEY,
        DEFAULT_RUN_GENESIS_REQUEST,
    },
    DEFAULT_ACCOUNT_ADDR,
};

use casper_execution_engine::core::{engine_state::Error, execution};
use casper_types::{
    runtime_args,
    system::{
        self,
        auction::{self, DelegationRate},
    },
    ApiError, ContractHash, ContractWasmHash, Key, RuntimeArgs, U512,
};
use parity_wasm::elements::{Module, Section};

const CONTRACT_FACTORY_WASM: &str = "contract_factory.wasm";

#[ignore]
#[test]
fn should_run_contract_factory() {
    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&*DEFAULT_RUN_GENESIS_REQUEST);

    let install_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_FACTORY_WASM,
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(install_request).expect_success().commit();
    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have default account");
    let contract_hash = account
        .named_keys()
        .get("contract_hash")
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have contract hash");
    let contract = builder
        .get_contract(contract_hash)
        .expect("should have contract");
    let contract_wasm_hash = contract
        .contract_wasm_key()
        .into_hash()
        .map(ContractWasmHash::new)
        .expect("should have contract wasm hash");
    let contract_wasm = builder
        .get_contract_wasm(contract_wasm_hash)
        .expect("should have wasm");
    let contract_wasm_bytes = contract_wasm.take_bytes();

    let module = parity_wasm::deserialize_buffer::<Module>(&contract_wasm_bytes)
        .expect("should deserialize wasm");

    let _imports: BTreeSet<&str> = module
        .sections()
        .iter()
        .filter_map(|section| {
            if let Section::Import(import_section) = section {
                Some(import_section)
            } else {
                None
            }
        })
        .map(|import_section| import_section.entries())
        .flatten()
        .map(|entry| entry.field())
        .collect();

    
}
