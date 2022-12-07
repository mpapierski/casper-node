//! A library to support testing of Wasm smart contracts for use on the Casper Platform.

#![doc(html_root_url = "https://docs.rs/casper-engine-test-support/2.2.0")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/CasperLabs/casper-node/master/images/CasperLabs_Logo_Favicon_RGB_50px.png",
    html_logo_url = "https://raw.githubusercontent.com/CasperLabs/casper-node/master/images/CasperLabs_Logo_Symbol_RGB.png",
    test(attr(forbid(warnings)))
)]
#![warn(missing_docs)]
mod additive_map_diff;
/// Utility methods for running the auction in a test or bench context.
pub mod auction;
mod chainspec_config;
mod deploy_item_builder;
mod execute_request_builder;
mod step_request_builder;
/// Utilities for running transfers in a test or bench context.
pub mod transfer;
mod upgrade_request_builder;
pub mod utils;
mod wasm_test_builder;

use std::fmt::Display;

use num_rational::Ratio;
use once_cell::sync::Lazy;

use casper_execution_engine::{
    core::engine_state::{
        ChainspecRegistry, EngineConfig, ExecConfig, ExecuteRequest, GenesisAccount, GenesisConfig,
        RunGenesisRequest, DEFAULT_MAX_QUERY_DEPTH,
    },
    shared::{system_config::SystemConfig, wasm_config::WasmConfig, wasm_engine::ExecutionMode},
};
use casper_hashing::Digest;
use casper_types::{account::AccountHash, Motes, ProtocolVersion, PublicKey, SecretKey, U512};

pub use crate::chainspec_config::PRODUCTION_CHAINSPEC;
use crate::chainspec_config::PRODUCTION_PATH;
pub use additive_map_diff::AdditiveMapDiff;
pub use chainspec_config::ChainspecConfig;
pub use deploy_item_builder::DeployItemBuilder;
pub use execute_request_builder::ExecuteRequestBuilder;
pub use step_request_builder::StepRequestBuilder;
pub use upgrade_request_builder::UpgradeRequestBuilder;
pub use wasm_test_builder::{InMemoryWasmTestBuilder, LmdbWasmTestBuilder, WasmTestBuilder};

#[macro_export]
macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);

        let mut tokens = name.rsplit("::");
        assert_eq!(tokens.next(), Some("f"));
        tokens.next().unwrap()
    }};
}

#[derive(Clone)]
pub struct Instrumented<'a> {
    module_name: &'a str,
    file: &'a str,
    line: u32,
    function: &'a str,
    data: Option<(&'a str, String)>,
}

impl<'a> Instrumented<'a> {
    pub fn new<T: Display>(
        module_name: &'a str,
        file: &'a str,
        line: u32,
        function: &'a str,
        data: Option<(&'a str, T)>,
    ) -> Self {
        Self {
            module_name,
            file,
            line,
            function,
            data: data.map(|(key, value)| (key, value.to_string())),
        }
    }

    pub fn with_function(&self, new_function: &'a str) -> Self {
        Self {
            module_name: self.module_name,
            file: self.file,
            line: self.line,
            function: new_function,
            data: self.data.clone(),
        }
    }
}

#[macro_export]
macro_rules! instrumentation_data {
    ( ) => {{
        $crate::Instrumented::new(
            module_path!(),
            file!(),
            line!(),
            $crate::function!(),
            Option::<(&str, &str)>::None,
        )
    }};
    ($val:expr) => {{
        $crate::Instrumented::new(
            module_path!(),
            file!(),
            line!(),
            $crate::function!(),
            Some((stringify!($val), $val)),
        )
    }};
}

/// Wraps an expression with details useful for instrumentation.
#[macro_export]
macro_rules! instrumented {
    ( $x:expr) => {{
        ($x, $crate::instrumentation_data!())
    }};
    ( $x:expr, $val:expr) => {{
        ($x, $crate::instrumentation_data!($val))
    }};
}

const DAY_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// Default number of validator slots.
pub const DEFAULT_VALIDATOR_SLOTS: u32 = 5;
/// Default auction delay.
pub const DEFAULT_AUCTION_DELAY: u64 = 3;
/// Default lock-in period of 90 days
pub const DEFAULT_LOCKED_FUNDS_PERIOD_MILLIS: u64 = 90 * DAY_MILLIS;
/// Default length of total vesting schedule of 91 days.
pub const DEFAULT_VESTING_SCHEDULE_PERIOD_MILLIS: u64 = 91 * DAY_MILLIS;

/// Default number of eras that need to pass to be able to withdraw unbonded funds.
pub const DEFAULT_UNBONDING_DELAY: u64 = 14;

/// Default round seigniorage rate represented as a fractional number.
///
/// Annual issuance: 2%
/// Minimum round exponent: 14
/// Ticks per year: 31536000000
///
/// (1+0.02)^((2^14)/31536000000)-1 is expressed as a fraction below.
pub const DEFAULT_ROUND_SEIGNIORAGE_RATE: Ratio<u64> = Ratio::new_raw(6414, 623437335209);

/// Default chain name.
pub const DEFAULT_CHAIN_NAME: &str = "casper-execution-engine-testing";
/// Default genesis timestamp in milliseconds.
pub const DEFAULT_GENESIS_TIMESTAMP_MILLIS: u64 = 0;
/// Default maximum number of associated keys.
pub const DEFAULT_MAX_ASSOCIATED_KEYS: u32 = 100;
/// Default max serialized size of `StoredValue`s.
#[deprecated(
    since = "2.3.0",
    note = "not used in `casper-execution-engine` config anymore"
)]
pub const DEFAULT_MAX_STORED_VALUE_SIZE: u32 = 8 * 1024 * 1024;
/// Default block time.
pub const DEFAULT_BLOCK_TIME: u64 = 0;
/// Default gas price.
pub const DEFAULT_GAS_PRICE: u64 = 1;

/// Amount named argument.
pub const ARG_AMOUNT: &str = "amount";
/// Timestamp increment in milliseconds.
pub const TIMESTAMP_MILLIS_INCREMENT: u64 = 30_000; // 30 seconds

/// Default genesis config hash.
pub static DEFAULT_GENESIS_CONFIG_HASH: Lazy<Digest> = Lazy::new(|| [42; 32].into());
/// Default account public key.
pub static DEFAULT_ACCOUNT_PUBLIC_KEY: Lazy<PublicKey> = Lazy::new(|| {
    let secret_key = SecretKey::ed25519_from_bytes([199; SecretKey::ED25519_LENGTH]).unwrap();
    PublicKey::from(&secret_key)
});
/// Default test account address.
pub static DEFAULT_ACCOUNT_ADDR: Lazy<AccountHash> =
    Lazy::new(|| AccountHash::from(&*DEFAULT_ACCOUNT_PUBLIC_KEY));
// NOTE: declaring DEFAULT_ACCOUNT_KEY as *DEFAULT_ACCOUNT_ADDR causes tests to stall.
/// Default account key.
pub static DEFAULT_ACCOUNT_KEY: Lazy<AccountHash> =
    Lazy::new(|| AccountHash::from(&*DEFAULT_ACCOUNT_PUBLIC_KEY));
/// Default initial balance of a test account in motes.
pub const DEFAULT_ACCOUNT_INITIAL_BALANCE: u64 = 100_000_000_000_000_000u64;
/// Minimal amount for a transfer that creates new accounts.
pub const MINIMUM_ACCOUNT_CREATION_BALANCE: u64 = 7_500_000_000_000_000u64;
/// Default proposer public key.
pub static DEFAULT_PROPOSER_PUBLIC_KEY: Lazy<PublicKey> = Lazy::new(|| {
    let secret_key = SecretKey::ed25519_from_bytes([198; SecretKey::ED25519_LENGTH]).unwrap();
    PublicKey::from(&secret_key)
});
/// Default proposer address.
pub static DEFAULT_PROPOSER_ADDR: Lazy<AccountHash> =
    Lazy::new(|| AccountHash::from(&*DEFAULT_PROPOSER_PUBLIC_KEY));
/// Default accounts.
pub static DEFAULT_ACCOUNTS: Lazy<Vec<GenesisAccount>> = Lazy::new(|| {
    let mut ret = Vec::new();
    let genesis_account = GenesisAccount::account(
        DEFAULT_ACCOUNT_PUBLIC_KEY.clone(),
        Motes::new(DEFAULT_ACCOUNT_INITIAL_BALANCE.into()),
        None,
    );
    ret.push(genesis_account);
    let proposer_account = GenesisAccount::account(
        DEFAULT_PROPOSER_PUBLIC_KEY.clone(),
        Motes::new(DEFAULT_ACCOUNT_INITIAL_BALANCE.into()),
        None,
    );
    ret.push(proposer_account);
    ret
});
/// Default [`ProtocolVersion`].
pub static DEFAULT_PROTOCOL_VERSION: Lazy<ProtocolVersion> = Lazy::new(|| ProtocolVersion::V1_0_0);
/// Default payment.
pub static DEFAULT_PAYMENT: Lazy<U512> = Lazy::new(|| U512::from(1_500_000_000_000u64));
/// Default [`WasmConfig`].
pub static DEFAULT_WASM_CONFIG: Lazy<WasmConfig> = Lazy::new(WasmConfig::default);
/// Default [`SystemConfig`].
pub static DEFAULT_SYSTEM_CONFIG: Lazy<SystemConfig> = Lazy::new(SystemConfig::default);
/// Default [`ExecConfig`].
pub static DEFAULT_EXEC_CONFIG: Lazy<ExecConfig> = Lazy::new(|| {
    ExecConfig::new(
        DEFAULT_ACCOUNTS.clone(),
        *DEFAULT_WASM_CONFIG,
        *DEFAULT_SYSTEM_CONFIG,
        DEFAULT_VALIDATOR_SLOTS,
        DEFAULT_AUCTION_DELAY,
        DEFAULT_LOCKED_FUNDS_PERIOD_MILLIS,
        DEFAULT_ROUND_SEIGNIORAGE_RATE,
        DEFAULT_UNBONDING_DELAY,
        DEFAULT_GENESIS_TIMESTAMP_MILLIS,
    )
});
/// Default [`GenesisConfig`].
pub static DEFAULT_GENESIS_CONFIG: Lazy<GenesisConfig> = Lazy::new(|| {
    GenesisConfig::new(
        DEFAULT_CHAIN_NAME.to_string(),
        DEFAULT_GENESIS_TIMESTAMP_MILLIS,
        *DEFAULT_PROTOCOL_VERSION,
        #[allow(deprecated)]
        DEFAULT_EXEC_CONFIG.clone(),
    )
});
/// Default [`ChainspecRegistry`].
pub static DEFAULT_CHAINSPEC_REGISTRY: Lazy<ChainspecRegistry> =
    Lazy::new(|| ChainspecRegistry::new_with_genesis(&[1, 2, 3], &[4, 5, 6]));
// This static constant has been deprecated in favor of the Production counterpart
// which uses costs tables and values which reflect values used by the Casper Mainnet.
#[deprecated]
/// Default [`RunGenesisRequest`].
pub static DEFAULT_RUN_GENESIS_REQUEST: Lazy<RunGenesisRequest> = Lazy::new(|| {
    RunGenesisRequest::new(
        *DEFAULT_GENESIS_CONFIG_HASH,
        *DEFAULT_PROTOCOL_VERSION,
        #[allow(deprecated)]
        DEFAULT_EXEC_CONFIG.clone(),
        DEFAULT_CHAINSPEC_REGISTRY.clone(),
    )
});
/// [`RunGenesisRequest`] instantiated using chainspec values.
pub static PRODUCTION_RUN_GENESIS_REQUEST: Lazy<RunGenesisRequest> = Lazy::new(|| {
    ChainspecConfig::create_genesis_request_from_production_chainspec(
        DEFAULT_ACCOUNTS.clone(),
        *DEFAULT_PROTOCOL_VERSION,
    )
    .expect("must create the request")
});
/// Round seigniorage rate from the production chainspec.
pub static PRODUCTION_ROUND_SEIGNIORAGE_RATE: Lazy<Ratio<u64>> = Lazy::new(|| {
    let chainspec = ChainspecConfig::from_chainspec_path(&*PRODUCTION_PATH)
        .expect("must create chainspec_config");
    chainspec.core_config.round_seigniorage_rate
});
pub static PRODUCTION_ENGINE_CONFIG: Lazy<EngineConfig> = Lazy::new(|| {
    let chainspec = ChainspecConfig::from_chainspec_path(&*PRODUCTION_PATH)
        .expect("must create chainspec_config");
    EngineConfig::new(
        DEFAULT_MAX_QUERY_DEPTH,
        chainspec.core_config.max_associated_keys,
        chainspec.core_config.max_runtime_call_stack_height,
        chainspec.core_config.minimum_delegation_amount,
        chainspec.core_config.strict_argument_checking,
        chainspec.core_config.vesting_schedule_period,
        chainspec.wasm_config,
        chainspec.system_costs_config,
    )
});
/// System address.
pub static SYSTEM_ADDR: Lazy<AccountHash> = Lazy::new(|| PublicKey::System.to_account_hash());

#[cfg(test)]
mod tests {
    use crate::function;

    #[test]
    fn testname() {
        let name = function!();
        assert_eq!(name, "testname");
        let closure = || function!();
        assert_eq!(closure(), "{{closure}}");
    }
}
