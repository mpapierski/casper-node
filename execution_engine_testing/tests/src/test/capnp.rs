use num_traits::Zero;

use casper_engine_test_support::{
    DeployItemBuilder, ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_ACCOUNT_ADDR,
    DEFAULT_RUN_GENESIS_REQUEST,
};
use casper_execution_engine::{
    core::engine_state::{Error as CoreError, MAX_PAYMENT},
    shared::{opcode_costs::DEFAULT_NOP_COST, wasm},
};
use casper_types::{
    bytesrepr::{self, Bytes},
    contracts::DEFAULT_ENTRY_POINT_NAME,
    runtime_args, ContractHash, Gas, RuntimeArgs, U512,
};
use parity_wasm::{
    builder,
    elements::{Instruction, Instructions},
};

const ARG_AMOUNT: &str = "amount";
const EXPECTED_U512: U512 = U512([20, 67, 64, 96, 209, 102, 99, 158]);
#[ignore]
#[test]
fn addressbook_measure_capnp() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_capnp",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();

    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();
    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}

#[ignore]
#[test]
fn addressbook_measure_borsh() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());
    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_borsh",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();

    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();

    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}

#[ignore]
#[test]
fn addressbook_measure_tobytes() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_tobytes",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();

    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();

    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}

#[ignore]
#[test]
fn u512_measure_capnp() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_simple_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_capnp",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();

    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();

    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}

#[ignore]
#[test]
fn u512_measure_borsh() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_simple_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_borsh",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();

    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();

    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}

#[ignore]
#[test]
fn u512_measure_tobytes() {
    let minimum_deploy_payment = U512::from(0u64);

    let mut builder = InMemoryWasmTestBuilder::default();

    builder.run_genesis(&DEFAULT_RUN_GENESIS_REQUEST);

    let exec_request = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        "serialization_simple_stored.wasm",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request).expect_success().commit();

    let account = builder.get_account(*DEFAULT_ACCOUNT_ADDR).unwrap();

    let proposer_balance_before = builder.get_proposer_purse_balance();

    let account_balance_before = builder.get_purse_balance(account.main_purse());

    let exec_request_2 = ExecuteRequestBuilder::contract_call_by_name(
        *DEFAULT_ACCOUNT_ADDR,
        "do_nothing_hash",
        "write_tobytes",
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(exec_request_2).expect_success().commit();
    let contract_hash = account.named_keys()["do_nothing_hash"]
        .into_hash()
        .map(ContractHash::new)
        .unwrap();
    let value_bytes = builder.get_value::<Bytes>(contract_hash, "storage");

    let contract = builder.get_contract(contract_hash).unwrap();
    let contract_wasm = builder
        .get_contract_wasm(contract.contract_wasm_hash())
        .unwrap();

    let gas = builder.last_exec_gas_cost();
    assert_eq!(
        (value_bytes.len(), contract_wasm.bytes().len(), gas),
        (0usize, 0usize, Gas::zero()),
        "datalen, wasm, gas"
    );
}
