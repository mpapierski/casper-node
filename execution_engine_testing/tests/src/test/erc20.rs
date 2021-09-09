use once_cell::sync::Lazy;

use casper_engine_test_support::{
    internal::{ExecuteRequestBuilder, InMemoryWasmTestBuilder, DEFAULT_RUN_GENESIS_REQUEST},
    DEFAULT_ACCOUNT_ADDR, MINIMUM_ACCOUNT_CREATION_BALANCE,
};
use casper_execution_engine::core::{
    engine_state::{Error as CoreError, ExecuteRequest},
    execution::Error as ExecError,
};
use casper_types::{
    account::AccountHash, runtime_args, system::mint, ApiError, ContractHash, Key, PublicKey,
    RuntimeArgs, SecretKey, U256,
};

const EXAMPLE_ERC20_TOKEN: &str = "erc20_token.wasm";
const CONTRACT_ERC20_TEST: &str = "erc20_test.wasm";
const CONTRACT_ERC20_TEST_CALL: &str = "erc20_test_call.wasm";
const NAME_KEY: &str = "name";
const SYMBOL_KEY: &str = "symbol";
const ERC20_TOKEN_CONTRACT_KEY: &str = "erc20_token_contract";
const DECIMALS_KEY: &str = "decimals";
const TOTAL_SUPPLY_KEY: &str = "total_supply";
const BALANCES_KEY: &str = "balances";
const ALLOWANCES_KEY: &str = "allowances";

const ARG_NAME: &str = "name";
const ARG_SYMBOL: &str = "symbol";
const ARG_DECIMALS: &str = "decimals";
const ARG_TOTAL_SUPPLY: &str = "total_supply";

const TEST_CONTRACT_KEY: &str = "test_contract";

const _ERROR_INVALID_CONTEXT: u16 = u16::MAX;
const ERROR_INSUFFICIENT_BALANCE: u16 = u16::MAX - 1;
const ERROR_INSUFFICIENT_ALLOWANCE: u16 = u16::MAX - 2;
const _ERROR_OVERFLOW: u16 = u16::MAX - 3;

const TOKEN_NAME: &str = "CasperTest";
const TOKEN_SYMBOL: &str = "CSPRT";
const TOKEN_DECIMALS: u8 = 100;
const TOKEN_TOTAL_SUPPLY: u64 = 1_000_000_000;

const METHOD_TRANSFER: &str = "transfer";
const ARG_AMOUNT: &str = "amount";
const ARG_RECIPIENT: &str = "recipient";

const METHOD_APPROVE: &str = "approve";
const ARG_OWNER: &str = "owner";
const ARG_SPENDER: &str = "spender";

const METHOD_TRANSFER_FROM: &str = "transfer_from";

const CHECK_BALANCE_OF_ENTRYPOINT: &str = "check_balance_of";
const CHECK_ALLOWANCE_OF_ENTRYPOINT: &str = "check_allowance_of";

const ARG_TOKEN_CONTRACT: &str = "token_contract";
const ARG_ADDRESS: &str = "address";
const RESULT_KEY: &str = "result";
const ERC20_TEST_CALL_KEY: &str = "erc20_test_call";

static ACCOUNT_1_SECRET_KEY: Lazy<SecretKey> =
    Lazy::new(|| SecretKey::secp256k1_from_bytes(&[221u8; 32]).unwrap());
static ACCOUNT_1_PUBLIC_KEY: Lazy<PublicKey> =
    Lazy::new(|| PublicKey::from(&*ACCOUNT_1_SECRET_KEY));
static ACCOUNT_1_ADDR: Lazy<AccountHash> = Lazy::new(|| ACCOUNT_1_PUBLIC_KEY.to_account_hash());

static ACCOUNT_2_SECRET_KEY: Lazy<SecretKey> =
    Lazy::new(|| SecretKey::secp256k1_from_bytes(&[212u8; 32]).unwrap());
static ACCOUNT_2_PUBLIC_KEY: Lazy<PublicKey> =
    Lazy::new(|| PublicKey::from(&*ACCOUNT_2_SECRET_KEY));
static ACCOUNT_2_ADDR: Lazy<AccountHash> = Lazy::new(|| ACCOUNT_2_PUBLIC_KEY.to_account_hash());

const TRANSFER_AMOUNT_1: u64 = 200_001;
const TRANSFER_AMOUNT_2: u64 = 19_999;
const ALLOWANCE_AMOUNT_1: u64 = 456_789;
const ALLOWANCE_AMOUNT_2: u64 = 87_654;

const METHOD_TRANSFER_AS_STORED_CONTRACT: &str = "transfer_as_stored_contract";

fn invert_erc20_address(address: Key) -> Key {
    match address {
        Key::Account(account_hash) => Key::Hash(account_hash.value()),
        Key::Hash(contract_hash) => Key::Account(AccountHash::new(contract_hash)),
        _ => panic!("Unsupported Key variant"),
    }
}

#[derive(Copy, Clone)]
struct TestContext {
    erc20_token: ContractHash,
    test_contract: ContractHash,
    erc20_test_call: ContractHash,
}

fn setup() -> (InMemoryWasmTestBuilder, TestContext) {
    let mut builder = InMemoryWasmTestBuilder::default();
    builder.run_genesis(&*DEFAULT_RUN_GENESIS_REQUEST);

    let id: Option<u64> = None;
    let transfer_1_args = runtime_args! {
        mint::ARG_TARGET => *ACCOUNT_1_ADDR,
        mint::ARG_AMOUNT => MINIMUM_ACCOUNT_CREATION_BALANCE,
        mint::ARG_ID => id,
    };
    let transfer_2_args = runtime_args! {
        mint::ARG_TARGET => *ACCOUNT_2_ADDR,
        mint::ARG_AMOUNT => MINIMUM_ACCOUNT_CREATION_BALANCE,
        mint::ARG_ID => id,
    };

    let transfer_request_1 =
        ExecuteRequestBuilder::transfer(*DEFAULT_ACCOUNT_ADDR, transfer_1_args).build();
    let transfer_request_2 =
        ExecuteRequestBuilder::transfer(*DEFAULT_ACCOUNT_ADDR, transfer_2_args).build();

    let install_request_1 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        EXAMPLE_ERC20_TOKEN,
        runtime_args! {
            ARG_NAME => TOKEN_NAME,
            ARG_SYMBOL => TOKEN_SYMBOL,
            ARG_DECIMALS => TOKEN_DECIMALS,
            ARG_TOTAL_SUPPLY => U256::from(TOKEN_TOTAL_SUPPLY),
        },
    )
    .build();
    let install_request_2 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_ERC20_TEST,
        RuntimeArgs::default(),
    )
    .build();
    let install_request_3 = ExecuteRequestBuilder::standard(
        *DEFAULT_ACCOUNT_ADDR,
        CONTRACT_ERC20_TEST_CALL,
        RuntimeArgs::default(),
    )
    .build();

    builder.exec(transfer_request_1).expect_success().commit();
    builder.exec(transfer_request_2).expect_success().commit();
    builder.exec(install_request_1).expect_success().commit();
    builder.exec(install_request_2).expect_success().commit();
    builder.exec(install_request_3).expect_success().commit();

    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have account");

    let erc20_token = account
        .named_keys()
        .get(ERC20_TOKEN_CONTRACT_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have contract hash");

    let test_contract = account
        .named_keys()
        .get(TEST_CONTRACT_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have contract hash");

    let erc20_test_call = account
        .named_keys()
        .get(ERC20_TEST_CALL_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have contract hash");

    let test_context = TestContext {
        erc20_token,
        test_contract,
        erc20_test_call,
    };

    (builder, test_context)
}

#[ignore]
#[test]
fn should_have_queryable_properties() {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let name: String = builder.get_value(erc20_token, NAME_KEY);
    assert_eq!(name, TOKEN_NAME);

    let symbol: String = builder.get_value(erc20_token, SYMBOL_KEY);
    assert_eq!(symbol, TOKEN_SYMBOL);

    let decimals: u8 = builder.get_value(erc20_token, DECIMALS_KEY);
    assert_eq!(decimals, TOKEN_DECIMALS);

    let total_supply: U256 = builder.get_value(erc20_token, TOTAL_SUPPLY_KEY);
    assert_eq!(total_supply, U256::from(TOKEN_TOTAL_SUPPLY));

    let owner_key = Key::Account(*DEFAULT_ACCOUNT_ADDR);

    let owner_balance = erc20_check_balance_of(&mut builder, owner_key);
    assert_eq!(owner_balance, total_supply);

    let contract_balance = erc20_check_balance_of(&mut builder, Key::Hash(erc20_token.value()));
    assert_eq!(contract_balance, U256::zero());

    // Ensures that Account and Contract ownership is respected and we're not keying ownership under
    // the bytes
    let inverted_owner_key = invert_erc20_address(owner_key);
    let inverted_owner_balance = erc20_check_balance_of(&mut builder, inverted_owner_key);
    assert_eq!(inverted_owner_balance, U256::zero());
}

#[ignore]
#[test]
fn should_not_have_balances_or_allowances_after_install() {
    let (builder, _contract_hash) = setup();

    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have account");

    let named_keys = account.named_keys();
    assert!(!named_keys.contains_key(BALANCES_KEY), "{:?}", named_keys);
    assert!(!named_keys.contains_key(ALLOWANCES_KEY), "{:?}", named_keys);
}

fn erc20_check_balance_of(builder: &mut InMemoryWasmTestBuilder, address: Key) -> U256 {
    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have account");
    let erc20_contract_hash = account
        .named_keys()
        .get(ERC20_TOKEN_CONTRACT_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have test contract hash");
    let erc20_test_contract_hash = account
        .named_keys()
        .get(ERC20_TEST_CALL_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have test contract hash");

    let check_balance_args = runtime_args! {
        ARG_TOKEN_CONTRACT => erc20_contract_hash,
        ARG_ADDRESS => address,
    };
    let exec_request = ExecuteRequestBuilder::contract_call_by_hash(
        *DEFAULT_ACCOUNT_ADDR,
        erc20_test_contract_hash,
        CHECK_BALANCE_OF_ENTRYPOINT,
        check_balance_args,
    )
    .build();
    builder.exec(exec_request).expect_success().commit();

    builder.get_value(erc20_test_contract_hash, RESULT_KEY)
}

fn erc20_check_allowance_of(
    builder: &mut InMemoryWasmTestBuilder,
    owner: Key,
    spender: Key,
) -> U256 {
    let account = builder
        .get_account(*DEFAULT_ACCOUNT_ADDR)
        .expect("should have account");
    let erc20_contract_hash = account
        .named_keys()
        .get(ERC20_TOKEN_CONTRACT_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have test contract hash");
    let erc20_test_contract_hash = account
        .named_keys()
        .get(ERC20_TEST_CALL_KEY)
        .and_then(|key| key.into_hash())
        .map(ContractHash::new)
        .expect("should have test contract hash");

    let check_balance_args = runtime_args! {
        ARG_TOKEN_CONTRACT => erc20_contract_hash,
        ARG_OWNER => owner,
        ARG_SPENDER => spender,
    };
    let exec_request = ExecuteRequestBuilder::contract_call_by_hash(
        *DEFAULT_ACCOUNT_ADDR,
        erc20_test_contract_hash,
        CHECK_ALLOWANCE_OF_ENTRYPOINT,
        check_balance_args,
    )
    .build();
    builder.exec(exec_request).expect_success().commit();

    builder.get_value(erc20_test_contract_hash, RESULT_KEY)
}

fn test_transfer_from_account(
    builder: &mut InMemoryWasmTestBuilder,
    test_context: &TestContext,
    sender1: Key,
    recipient1: Key,
    sender2: Key,
    recipient2: Key,
) {
    let TestContext { erc20_token, .. } = test_context;

    let transfer_amount_1 = U256::from(TRANSFER_AMOUNT_1);
    let transfer_amount_2 = U256::from(TRANSFER_AMOUNT_2);

    let sender_balance_before = erc20_check_balance_of(builder, sender1);
    assert_ne!(sender_balance_before, U256::zero());

    let account_1_balance_before = erc20_check_balance_of(builder, recipient1);
    assert_eq!(account_1_balance_before, U256::zero());

    let account_2_balance_before = erc20_check_balance_of(builder, recipient1);
    assert_eq!(account_2_balance_before, U256::zero());

    let token_transfer_request_1 =
        make_erc20_transfer_request(sender1, erc20_token, recipient1, transfer_amount_1);

    builder
        .exec(token_transfer_request_1)
        .expect_success()
        .commit();

    let account_1_balance_after = erc20_check_balance_of(builder, recipient1);
    assert_eq!(account_1_balance_after, transfer_amount_1);
    let account_1_balance_before = account_1_balance_after;

    let sender_balance_after = erc20_check_balance_of(builder, sender1);
    assert_eq!(
        sender_balance_after,
        sender_balance_before - transfer_amount_1
    );
    let sender_balance_before = sender_balance_after;

    let token_transfer_request_2 =
        make_erc20_transfer_request(sender2, erc20_token, recipient2, transfer_amount_2);

    builder
        .exec(token_transfer_request_2)
        .expect_success()
        .commit();

    let sender_balance_after = erc20_check_balance_of(builder, sender1);
    assert_eq!(sender_balance_after, sender_balance_before);

    let account_1_balance_after = erc20_check_balance_of(builder, recipient1);
    assert!(account_1_balance_after < account_1_balance_before);
    assert_eq!(
        account_1_balance_after,
        transfer_amount_1 - transfer_amount_2
    );

    let account_2_balance_after = erc20_check_balance_of(builder, recipient2);
    assert_eq!(account_2_balance_after, transfer_amount_2);
}

fn make_erc20_transfer_request(
    sender: Key,
    erc20_token: &ContractHash,
    recipient: Key,
    amount: U256,
) -> ExecuteRequest {
    match sender {
        Key::Account(sender) => ExecuteRequestBuilder::contract_call_by_hash(
            sender,
            *erc20_token,
            METHOD_TRANSFER,
            runtime_args! {
                ARG_AMOUNT => amount,
                ARG_RECIPIENT => recipient,
            },
        )
        .build(),
        Key::Hash(contract_hash) => ExecuteRequestBuilder::contract_call_by_hash(
            *DEFAULT_ACCOUNT_ADDR,
            ContractHash::new(contract_hash),
            METHOD_TRANSFER_AS_STORED_CONTRACT,
            runtime_args! {
                ARG_TOKEN_CONTRACT => *erc20_token,
                ARG_AMOUNT => amount,
                ARG_RECIPIENT => recipient,
            },
        )
        .build(),
        _ => panic!("Unknown variant"),
    }
}

#[ignore]
#[test]
fn should_transfer_from_account_to_account() {
    let (mut builder, test_context) = setup();
    let sender1 = Key::Account(*DEFAULT_ACCOUNT_ADDR);
    let recipient1 = Key::Account(*ACCOUNT_1_ADDR);
    let sender2 = Key::Account(*ACCOUNT_1_ADDR);
    let recipient2 = Key::Account(*ACCOUNT_2_ADDR);

    test_transfer_from_account(
        &mut builder,
        &test_context,
        sender1,
        recipient1,
        sender2,
        recipient2,
    );
}

#[ignore]
#[test]
fn should_transfer_from_account_to_contract() {
    let (mut builder, test_context) = setup();

    let sender1 = Key::Account(*DEFAULT_ACCOUNT_ADDR);
    let recipient1 = Key::Account(*ACCOUNT_1_ADDR);
    let sender2 = Key::Account(*ACCOUNT_1_ADDR);
    let recipient2 = Key::Hash(test_context.test_contract.value());

    test_transfer_from_account(
        &mut builder,
        &test_context,
        sender1,
        recipient1,
        sender2,
        recipient2,
    );
}

#[ignore]
#[test]
fn should_transfer_from_contract_to_contract() {
    let (mut builder, test_context) = setup();
    let TestContext {
        erc20_test_call, ..
    } = test_context;

    let sender1 = Key::Account(*DEFAULT_ACCOUNT_ADDR);
    let recipient1 = Key::Hash(erc20_test_call.value());
    let sender2 = Key::Hash(erc20_test_call.value());
    let recipient2 = Key::Hash([42; 32]);

    test_transfer_from_account(
        &mut builder,
        &test_context,
        sender1,
        recipient1,
        sender2,
        recipient2,
    );
}

#[ignore]
#[test]
fn should_transfer_from_contract_to_account() {
    let (mut builder, test_context) = setup();
    let TestContext {
        erc20_test_call, ..
    } = test_context;

    let sender1 = Key::Account(*DEFAULT_ACCOUNT_ADDR);
    let recipient1 = Key::Hash(erc20_test_call.value());

    let sender2 = Key::Hash(erc20_test_call.value());
    let recipient2 = Key::Account(*ACCOUNT_1_ADDR);

    test_transfer_from_account(
        &mut builder,
        &test_context,
        sender1,
        recipient1,
        sender2,
        recipient2,
    );
}

#[ignore]
#[test]
fn should_transfer_full_owned_amount() {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let initial_supply = U256::from(TOKEN_TOTAL_SUPPLY);
    let transfer_amount_1 = initial_supply;

    let transfer_1_sender = *DEFAULT_ACCOUNT_ADDR;
    let erc20_transfer_1_args = runtime_args! {
        ARG_RECIPIENT => Key::Account(*ACCOUNT_1_ADDR),
        ARG_AMOUNT => transfer_amount_1,
    };

    let owner_balance_before =
        erc20_check_balance_of(&mut builder, Key::Account(*DEFAULT_ACCOUNT_ADDR));
    assert_eq!(owner_balance_before, initial_supply);

    let account_1_balance_before =
        erc20_check_balance_of(&mut builder, Key::Account(*ACCOUNT_1_ADDR));
    assert_eq!(account_1_balance_before, U256::zero());

    let token_transfer_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        transfer_1_sender,
        erc20_token,
        METHOD_TRANSFER,
        erc20_transfer_1_args,
    )
    .build();

    builder
        .exec(token_transfer_request_1)
        .expect_success()
        .commit();

    let account_1_balance_after =
        erc20_check_balance_of(&mut builder, Key::Account(*ACCOUNT_1_ADDR));
    assert_eq!(account_1_balance_after, transfer_amount_1);

    let owner_balance_after =
        erc20_check_balance_of(&mut builder, Key::Account(*DEFAULT_ACCOUNT_ADDR));
    assert_eq!(owner_balance_after, U256::zero());

    let total_supply: U256 = builder.get_value(erc20_token, TOTAL_SUPPLY_KEY);
    assert_eq!(total_supply, initial_supply);
}

#[ignore]
#[test]
fn should_not_transfer_more_than_owned_balance() {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let initial_supply = U256::from(TOKEN_TOTAL_SUPPLY);
    let transfer_amount = initial_supply + U256::one();

    let transfer_1_sender = *DEFAULT_ACCOUNT_ADDR;
    let transfer_1_recipient = *ACCOUNT_1_ADDR;

    let erc20_transfer_1_args = runtime_args! {
        ARG_RECIPIENT => Key::Account(transfer_1_recipient),
        ARG_AMOUNT => transfer_amount,
    };

    let owner_balance_before =
        erc20_check_balance_of(&mut builder, Key::Account(*DEFAULT_ACCOUNT_ADDR));
    assert_eq!(owner_balance_before, initial_supply);
    assert!(transfer_amount > owner_balance_before);

    let account_1_balance_before =
        erc20_check_balance_of(&mut builder, Key::Account(*ACCOUNT_1_ADDR));
    assert_eq!(account_1_balance_before, U256::zero());

    let token_transfer_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        transfer_1_sender,
        erc20_token,
        METHOD_TRANSFER,
        erc20_transfer_1_args,
    )
    .build();

    builder.exec(token_transfer_request_1).commit();

    let error = builder.get_error().expect("should have error");
    assert!(
        matches!(error, CoreError::Exec(ExecError::Revert(ApiError::User(user_error))) if user_error == ERROR_INSUFFICIENT_BALANCE),
        "{:?}",
        error
    );

    let account_1_balance_after =
        erc20_check_balance_of(&mut builder, Key::Account(transfer_1_recipient));
    assert_eq!(account_1_balance_after, account_1_balance_before);

    let owner_balance_after = erc20_check_balance_of(&mut builder, Key::Account(transfer_1_sender));
    assert_eq!(owner_balance_after, initial_supply);

    let total_supply: U256 = builder.get_value(erc20_token, TOTAL_SUPPLY_KEY);
    assert_eq!(total_supply, initial_supply);
}

fn test_approve_for(sender: AccountHash, owner: Key, spender: Key) {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let initial_supply = U256::from(TOKEN_TOTAL_SUPPLY);
    let allowance_amount_1 = U256::from(ALLOWANCE_AMOUNT_1);
    let allowance_amount_2 = U256::from(ALLOWANCE_AMOUNT_2);

    let erc20_approve_1_args = runtime_args! {
        ARG_SPENDER => spender,
        ARG_AMOUNT => allowance_amount_1,
    };
    let erc20_approve_2_args = runtime_args! {
        ARG_SPENDER => spender,
        ARG_AMOUNT => allowance_amount_2,
    };

    let spender_allowance_before = erc20_check_allowance_of(&mut builder, owner, spender);
    assert_eq!(spender_allowance_before, U256::zero());

    let approve_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        sender,
        erc20_token,
        METHOD_APPROVE,
        erc20_approve_1_args,
    )
    .build();

    let approve_request_2 = ExecuteRequestBuilder::contract_call_by_hash(
        sender,
        erc20_token,
        METHOD_APPROVE,
        erc20_approve_2_args,
    )
    .build();

    builder.exec(approve_request_1).expect_success().commit();

    {
        let account_1_allowance_after = erc20_check_allowance_of(&mut builder, owner, spender);
        assert_eq!(account_1_allowance_after, allowance_amount_1);

        let total_supply: U256 = builder.get_value(erc20_token, TOTAL_SUPPLY_KEY);
        assert_eq!(total_supply, initial_supply);
    }

    // Approve overwrites existing amount rather than increase it

    builder.exec(approve_request_2).expect_success().commit();

    let account_1_allowance_after = erc20_check_allowance_of(&mut builder, owner, spender);
    assert_eq!(account_1_allowance_after, allowance_amount_2);

    // Swap Key::Account into Hash and other way
    let inverted_spender_key = invert_erc20_address(spender);

    let inverted_spender_allowance =
        erc20_check_allowance_of(&mut builder, owner, inverted_spender_key);
    assert_eq!(inverted_spender_allowance, U256::zero());

    let total_supply: U256 = builder.get_value(erc20_token, TOTAL_SUPPLY_KEY);
    assert_eq!(total_supply, initial_supply);
}

#[ignore]
#[test]
fn should_approve_funds_account_to_account() {
    test_approve_for(
        *DEFAULT_ACCOUNT_ADDR,
        Key::Account(*DEFAULT_ACCOUNT_ADDR),
        Key::Account(*ACCOUNT_1_ADDR),
    );
}

#[ignore]
#[test]
fn should_approve_funds_account_to_contract() {
    test_approve_for(
        *DEFAULT_ACCOUNT_ADDR,
        Key::Account(*DEFAULT_ACCOUNT_ADDR),
        Key::Hash([42; 32]),
    );
}

#[ignore]
#[test]
fn should_not_transfer_from_without_enough_allowance() {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let allowance_amount_1 = U256::from(ALLOWANCE_AMOUNT_1);
    let transfer_from_amount_1 = allowance_amount_1 + U256::one();

    let sender = *DEFAULT_ACCOUNT_ADDR;
    let owner = sender;
    let recipient = *ACCOUNT_1_ADDR;

    let erc20_approve_args = runtime_args! {
        ARG_OWNER => Key::Account(owner),
        ARG_SPENDER => Key::Account(recipient),
        ARG_AMOUNT => allowance_amount_1,
    };
    let erc20_transfer_from_args = runtime_args! {
        ARG_OWNER => Key::Account(owner),
        ARG_RECIPIENT => Key::Account(recipient),
        ARG_AMOUNT => transfer_from_amount_1,
    };

    let spender_allowance_before =
        erc20_check_allowance_of(&mut builder, Key::Account(owner), Key::Account(recipient));
    assert_eq!(spender_allowance_before, U256::zero());

    let approve_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        sender,
        erc20_token,
        METHOD_APPROVE,
        erc20_approve_args,
    )
    .build();

    let transfer_from_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        sender,
        erc20_token,
        METHOD_TRANSFER_FROM,
        erc20_transfer_from_args,
    )
    .build();

    builder.exec(approve_request_1).expect_success().commit();

    let account_1_allowance_after =
        erc20_check_allowance_of(&mut builder, Key::Account(owner), Key::Account(recipient));
    assert_eq!(account_1_allowance_after, allowance_amount_1);

    builder.exec(transfer_from_request_1).commit();

    let error = builder.get_error().expect("should have error");
    assert!(
        matches!(error, CoreError::Exec(ExecError::Revert(ApiError::User(user_error))) if user_error == ERROR_INSUFFICIENT_ALLOWANCE),
        "{:?}",
        error
    );
}

#[ignore]
#[test]
fn should_transfer_from_from_account_to_account() {
    let (mut builder, TestContext { erc20_token, .. }) = setup();

    let initial_supply = U256::from(TOKEN_TOTAL_SUPPLY);
    let allowance_amount_1 = U256::from(ALLOWANCE_AMOUNT_1);
    let transfer_from_amount_1 = allowance_amount_1;

    let owner = *DEFAULT_ACCOUNT_ADDR;
    let spender = *ACCOUNT_1_ADDR;

    let erc20_approve_args = runtime_args! {
        ARG_OWNER => Key::Account(owner),
        ARG_SPENDER => Key::Account(spender),
        ARG_AMOUNT => allowance_amount_1,
    };
    let erc20_transfer_from_args = runtime_args! {
        ARG_OWNER => Key::Account(owner),
        ARG_RECIPIENT => Key::Account(spender),
        ARG_AMOUNT => transfer_from_amount_1,
    };

    let spender_allowance_before =
        erc20_check_allowance_of(&mut builder, Key::Account(owner), Key::Account(spender));
    assert_eq!(spender_allowance_before, U256::zero());

    let approve_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        owner,
        erc20_token,
        METHOD_APPROVE,
        erc20_approve_args,
    )
    .build();

    let transfer_from_request_1 = ExecuteRequestBuilder::contract_call_by_hash(
        spender,
        erc20_token,
        METHOD_TRANSFER_FROM,
        erc20_transfer_from_args,
    )
    .build();

    builder.exec(approve_request_1).expect_success().commit();

    let account_1_balance_before = erc20_check_balance_of(&mut builder, Key::Account(owner));
    assert_eq!(account_1_balance_before, initial_supply);

    let account_1_allowance_before =
        erc20_check_allowance_of(&mut builder, Key::Account(owner), Key::Account(spender));
    assert_eq!(account_1_allowance_before, allowance_amount_1);

    builder
        .exec(transfer_from_request_1)
        .expect_success()
        .commit();

    let account_1_allowance_after =
        erc20_check_allowance_of(&mut builder, Key::Account(owner), Key::Account(spender));
    assert_eq!(
        account_1_allowance_after,
        account_1_allowance_before - transfer_from_amount_1
    );

    let account_1_balance_after = erc20_check_balance_of(&mut builder, Key::Account(owner));
    assert_eq!(
        account_1_balance_after,
        account_1_balance_before - transfer_from_amount_1
    );
}
