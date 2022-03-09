//! Support for host function gas cost tables.
use datasize::DataSize;
use rand::{distributions::Standard, prelude::Distribution, Rng};
use serde::{Deserialize, Serialize};

use casper_types::{
    bytesrepr::{self, BorshDeserialize, BorshSerialize, U32_SERIALIZED_LENGTH},
    Gas,
};

/// Representation of argument's cost.
pub type Cost = u32;

const COST_SERIALIZED_LENGTH: usize = U32_SERIALIZED_LENGTH;

/// An identifier that represents an unused argument.
const NOT_USED: Cost = 0;

/// An arbitrary default fixed cost for host functions that were not researched yet.
const DEFAULT_FIXED_COST: Cost = 200;

const DEFAULT_ADD_ASSOCIATED_KEY_COST: u32 = 9_000;
const DEFAULT_ADD_COST: u32 = 5_800;

const DEFAULT_CALL_CONTRACT_COST: u32 = 4_500;
const DEFAULT_CALL_CONTRACT_ARGS_SIZE_WEIGHT: u32 = 420;

const DEFAULT_CREATE_PURSE_COST: u32 = 2_500_000_000;
const DEFAULT_GET_BALANCE_COST: u32 = 3_800;
const DEFAULT_GET_BLOCKTIME_COST: u32 = 330;
const DEFAULT_GET_CALLER_COST: u32 = 380;
const DEFAULT_GET_KEY_COST: u32 = 2_000;
const DEFAULT_GET_KEY_NAME_SIZE_WEIGHT: u32 = 440;
const DEFAULT_GET_MAIN_PURSE_COST: u32 = 1_300;
const DEFAULT_GET_PHASE_COST: u32 = 710;
const DEFAULT_GET_SYSTEM_CONTRACT_COST: u32 = 1_100;
const DEFAULT_HAS_KEY_COST: u32 = 1_500;
const DEFAULT_HAS_KEY_NAME_SIZE_WEIGHT: u32 = 840;
const DEFAULT_IS_VALID_UREF_COST: u32 = 760;
const DEFAULT_LOAD_NAMED_KEYS_COST: u32 = 42_000;
const DEFAULT_NEW_UREF_COST: u32 = 17_000;
const DEFAULT_NEW_UREF_VALUE_SIZE_WEIGHT: u32 = 590;

const DEFAULT_PRINT_COST: u32 = 20_000;
const DEFAULT_PRINT_TEXT_SIZE_WEIGHT: u32 = 4_600;

const DEFAULT_PUT_KEY_COST: u32 = 38_000;
const DEFAULT_PUT_KEY_NAME_SIZE_WEIGHT: u32 = 1_100;

const DEFAULT_READ_HOST_BUFFER_COST: u32 = 3_500;
const DEFAULT_READ_HOST_BUFFER_DEST_SIZE_WEIGHT: u32 = 310;

const DEFAULT_READ_VALUE_COST: u32 = 6_000;
const DEFAULT_DICTIONARY_GET_COST: u32 = 5_500;
const DEFAULT_DICTIONARY_GET_KEY_SIZE_WEIGHT: u32 = 590;

const DEFAULT_REMOVE_ASSOCIATED_KEY_COST: u32 = 4_200;

const DEFAULT_REMOVE_KEY_COST: u32 = 61_000;
const DEFAULT_REMOVE_KEY_NAME_SIZE_WEIGHT: u32 = 3_200;

const DEFAULT_RET_COST: u32 = 23_000;
const DEFAULT_RET_VALUE_SIZE_WEIGHT: u32 = 420;

const DEFAULT_REVERT_COST: u32 = 500;
const DEFAULT_SET_ACTION_THRESHOLD_COST: u32 = 74_000;
const DEFAULT_TRANSFER_FROM_PURSE_TO_ACCOUNT_COST: u32 = 2_500_000_000;
const DEFAULT_TRANSFER_FROM_PURSE_TO_PURSE_COST: u32 = 82_000;
const DEFAULT_TRANSFER_TO_ACCOUNT_COST: u32 = 2_500_000_000;
const DEFAULT_UPDATE_ASSOCIATED_KEY_COST: u32 = 4_200;

const DEFAULT_WRITE_COST: u32 = 14_000;
const DEFAULT_WRITE_VALUE_SIZE_WEIGHT: u32 = 980;

const DEFAULT_DICTIONARY_PUT_COST: u32 = 9_500;
const DEFAULT_DICTIONARY_PUT_KEY_BYTES_SIZE_WEIGHT: u32 = 1_800;
const DEFAULT_DICTIONARY_PUT_VALUE_SIZE_WEIGHT: u32 = 520;

const DEFAULT_NEW_DICTIONARY_COST: u32 = DEFAULT_NEW_UREF_COST;

pub(crate) const DEFAULT_HOST_FUNCTION_NEW_DICTIONARY: HostFunction<[Cost; 1]> =
    HostFunction::new(DEFAULT_NEW_DICTIONARY_COST, [NOT_USED]);

/// Representation of a host function cost.
///
/// The total gas cost is equal to `cost` + sum of each argument weight multiplied by the byte size
/// of the data.
#[derive(
    Copy,
    Clone,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    Debug,
    DataSize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct HostFunction<T> {
    /// How much the user is charged for calling the host function.
    cost: Cost,
    /// Weights of the function arguments.
    arguments: T,
}

impl<T> Default for HostFunction<T>
where
    T: Default,
{
    fn default() -> Self {
        HostFunction::new(DEFAULT_FIXED_COST, Default::default())
    }
}

impl<T> HostFunction<T> {
    /// Creates a new instance of `HostFunction` with a fixed call cost and argument weights.
    pub const fn new(cost: Cost, arguments: T) -> Self {
        Self { cost, arguments }
    }

    /// Returns the base gas fee for calling the host function.
    pub fn cost(&self) -> Cost {
        self.cost
    }
}

impl<T> HostFunction<T>
where
    T: Default,
{
    /// Creates a new fixed host function cost with argument weights of zero.
    pub fn fixed(cost: Cost) -> Self {
        Self {
            cost,
            ..Default::default()
        }
    }
}

impl<T> HostFunction<T>
where
    T: AsRef<[Cost]>,
{
    /// Returns a slice containing the argument weights.
    pub fn arguments(&self) -> &[Cost] {
        self.arguments.as_ref()
    }

    /// Calculate gas cost for a host function
    pub fn calculate_gas_cost(&self, weights: T) -> Gas {
        let mut gas = Gas::new(self.cost.into());
        for (argument, weight) in self.arguments.as_ref().iter().zip(weights.as_ref()) {
            let lhs = Gas::new((*argument).into());
            let rhs = Gas::new((*weight).into());
            gas += lhs * rhs;
        }
        gas
    }
}

impl<T> Distribution<HostFunction<T>> for Standard
where
    Standard: Distribution<T>,
    T: AsRef<[Cost]>,
{
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> HostFunction<T> {
        let cost = rng.gen::<Cost>();
        let arguments = rng.gen();
        HostFunction::new(cost, arguments)
    }
}

/// Definition of a host function cost table.
#[derive(
    Copy,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Debug,
    DataSize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct HostFunctionCosts {
    /// Cost of calling the `read_value` host function.
    pub read_value: HostFunction<[Cost; 3]>,
    /// Cost of calling the `dictionary_get` host function.
    #[serde(alias = "read_value_local")]
    pub dictionary_get: HostFunction<[Cost; 3]>,
    /// Cost of calling the `write` host function.
    pub write: HostFunction<[Cost; 4]>,
    /// Cost of calling the `dictionary_put` host function.
    #[serde(alias = "write_local")]
    pub dictionary_put: HostFunction<[Cost; 4]>,
    /// Cost of calling the `add` host function.
    pub add: HostFunction<[Cost; 4]>,
    /// Cost of calling the `new_uref` host function.
    pub new_uref: HostFunction<[Cost; 3]>,
    /// Cost of calling the `load_named_keys` host function.
    pub load_named_keys: HostFunction<[Cost; 2]>,
    /// Cost of calling the `ret` host function.
    pub ret: HostFunction<[Cost; 2]>,
    /// Cost of calling the `get_key` host function.
    pub get_key: HostFunction<[Cost; 5]>,
    /// Cost of calling the `has_key` host function.
    pub has_key: HostFunction<[Cost; 2]>,
    /// Cost of calling the `put_key` host function.
    pub put_key: HostFunction<[Cost; 4]>,
    /// Cost of calling the `remove_key` host function.
    pub remove_key: HostFunction<[Cost; 2]>,
    /// Cost of calling the `revert` host function.
    pub revert: HostFunction<[Cost; 1]>,
    /// Cost of calling the `is_valid_uref` host function.
    pub is_valid_uref: HostFunction<[Cost; 2]>,
    /// Cost of calling the `add_associated_key` host function.
    pub add_associated_key: HostFunction<[Cost; 3]>,
    /// Cost of calling the `remove_associated_key` host function.
    pub remove_associated_key: HostFunction<[Cost; 2]>,
    /// Cost of calling the `update_associated_key` host function.
    pub update_associated_key: HostFunction<[Cost; 3]>,
    /// Cost of calling the `set_action_threshold` host function.
    pub set_action_threshold: HostFunction<[Cost; 2]>,
    /// Cost of calling the `get_caller` host function.
    pub get_caller: HostFunction<[Cost; 1]>,
    /// Cost of calling the `get_blocktime` host function.
    pub get_blocktime: HostFunction<[Cost; 1]>,
    /// Cost of calling the `create_purse` host function.
    pub create_purse: HostFunction<[Cost; 2]>,
    /// Cost of calling the `transfer_to_account` host function.
    pub transfer_to_account: HostFunction<[Cost; 7]>,
    /// Cost of calling the `transfer_from_purse_to_account` host function.
    pub transfer_from_purse_to_account: HostFunction<[Cost; 9]>,
    /// Cost of calling the `transfer_from_purse_to_purse` host function.
    pub transfer_from_purse_to_purse: HostFunction<[Cost; 8]>,
    /// Cost of calling the `get_balance` host function.
    pub get_balance: HostFunction<[Cost; 3]>,
    /// Cost of calling the `get_phase` host function.
    pub get_phase: HostFunction<[Cost; 1]>,
    /// Cost of calling the `get_system_contract` host function.
    pub get_system_contract: HostFunction<[Cost; 3]>,
    /// Cost of calling the `get_main_purse` host function.
    pub get_main_purse: HostFunction<[Cost; 1]>,
    /// Cost of calling the `read_host_buffer` host function.
    pub read_host_buffer: HostFunction<[Cost; 3]>,
    /// Cost of calling the `create_contract_package_at_hash` host function.
    pub create_contract_package_at_hash: HostFunction<[Cost; 2]>,
    /// Cost of calling the `create_contract_user_group` host function.
    pub create_contract_user_group: HostFunction<[Cost; 8]>,
    /// Cost of calling the `add_contract_version` host function.
    pub add_contract_version: HostFunction<[Cost; 10]>,
    /// Cost of calling the `disable_contract_version` host function.
    pub disable_contract_version: HostFunction<[Cost; 4]>,
    /// Cost of calling the `call_contract` host function.
    pub call_contract: HostFunction<[Cost; 7]>,
    /// Cost of calling the `call_versioned_contract` host function.
    pub call_versioned_contract: HostFunction<[Cost; 9]>,
    /// Cost of calling the `get_named_arg_size` host function.
    pub get_named_arg_size: HostFunction<[Cost; 3]>,
    /// Cost of calling the `get_named_arg` host function.
    pub get_named_arg: HostFunction<[Cost; 4]>,
    /// Cost of calling the `remove_contract_user_group` host function.
    pub remove_contract_user_group: HostFunction<[Cost; 4]>,
    /// Cost of calling the `provision_contract_user_group_uref` host function.
    pub provision_contract_user_group_uref: HostFunction<[Cost; 5]>,
    /// Cost of calling the `remove_contract_user_group_urefs` host function.
    pub remove_contract_user_group_urefs: HostFunction<[Cost; 6]>,
    /// Cost of calling the `print` host function.
    pub print: HostFunction<[Cost; 2]>,
    /// Cost of calling the `blake2b` host function.
    pub blake2b: HostFunction<[Cost; 4]>,
}

impl Default for HostFunctionCosts {
    fn default() -> Self {
        Self {
            read_value: HostFunction::fixed(DEFAULT_READ_VALUE_COST),
            dictionary_get: HostFunction::new(
                DEFAULT_DICTIONARY_GET_COST,
                [NOT_USED, DEFAULT_DICTIONARY_GET_KEY_SIZE_WEIGHT, NOT_USED],
            ),
            write: HostFunction::new(
                DEFAULT_WRITE_COST,
                [
                    NOT_USED,
                    NOT_USED,
                    NOT_USED,
                    DEFAULT_WRITE_VALUE_SIZE_WEIGHT,
                ],
            ),
            dictionary_put: HostFunction::new(
                DEFAULT_DICTIONARY_PUT_COST,
                [
                    NOT_USED,
                    DEFAULT_DICTIONARY_PUT_KEY_BYTES_SIZE_WEIGHT,
                    NOT_USED,
                    DEFAULT_DICTIONARY_PUT_VALUE_SIZE_WEIGHT,
                ],
            ),
            add: HostFunction::fixed(DEFAULT_ADD_COST),
            new_uref: HostFunction::new(
                DEFAULT_NEW_UREF_COST,
                [NOT_USED, NOT_USED, DEFAULT_NEW_UREF_VALUE_SIZE_WEIGHT],
            ),
            load_named_keys: HostFunction::fixed(DEFAULT_LOAD_NAMED_KEYS_COST),
            ret: HostFunction::new(DEFAULT_RET_COST, [NOT_USED, DEFAULT_RET_VALUE_SIZE_WEIGHT]),
            get_key: HostFunction::new(
                DEFAULT_GET_KEY_COST,
                [
                    NOT_USED,
                    DEFAULT_GET_KEY_NAME_SIZE_WEIGHT,
                    NOT_USED,
                    NOT_USED,
                    NOT_USED,
                ],
            ),
            has_key: HostFunction::new(
                DEFAULT_HAS_KEY_COST,
                [NOT_USED, DEFAULT_HAS_KEY_NAME_SIZE_WEIGHT],
            ),
            put_key: HostFunction::new(
                DEFAULT_PUT_KEY_COST,
                [
                    NOT_USED,
                    DEFAULT_PUT_KEY_NAME_SIZE_WEIGHT,
                    NOT_USED,
                    NOT_USED,
                ],
            ),
            remove_key: HostFunction::new(
                DEFAULT_REMOVE_KEY_COST,
                [NOT_USED, DEFAULT_REMOVE_KEY_NAME_SIZE_WEIGHT],
            ),
            revert: HostFunction::fixed(DEFAULT_REVERT_COST),
            is_valid_uref: HostFunction::fixed(DEFAULT_IS_VALID_UREF_COST),
            add_associated_key: HostFunction::fixed(DEFAULT_ADD_ASSOCIATED_KEY_COST),
            remove_associated_key: HostFunction::fixed(DEFAULT_REMOVE_ASSOCIATED_KEY_COST),
            update_associated_key: HostFunction::fixed(DEFAULT_UPDATE_ASSOCIATED_KEY_COST),
            set_action_threshold: HostFunction::fixed(DEFAULT_SET_ACTION_THRESHOLD_COST),
            get_caller: HostFunction::fixed(DEFAULT_GET_CALLER_COST),
            get_blocktime: HostFunction::fixed(DEFAULT_GET_BLOCKTIME_COST),
            create_purse: HostFunction::fixed(DEFAULT_CREATE_PURSE_COST),
            transfer_to_account: HostFunction::fixed(DEFAULT_TRANSFER_TO_ACCOUNT_COST),
            transfer_from_purse_to_account: HostFunction::fixed(
                DEFAULT_TRANSFER_FROM_PURSE_TO_ACCOUNT_COST,
            ),
            transfer_from_purse_to_purse: HostFunction::fixed(
                DEFAULT_TRANSFER_FROM_PURSE_TO_PURSE_COST,
            ),
            get_balance: HostFunction::fixed(DEFAULT_GET_BALANCE_COST),
            get_phase: HostFunction::fixed(DEFAULT_GET_PHASE_COST),
            get_system_contract: HostFunction::fixed(DEFAULT_GET_SYSTEM_CONTRACT_COST),
            get_main_purse: HostFunction::fixed(DEFAULT_GET_MAIN_PURSE_COST),
            read_host_buffer: HostFunction::new(
                DEFAULT_READ_HOST_BUFFER_COST,
                [
                    NOT_USED,
                    DEFAULT_READ_HOST_BUFFER_DEST_SIZE_WEIGHT,
                    NOT_USED,
                ],
            ),
            create_contract_package_at_hash: HostFunction::default(),
            create_contract_user_group: HostFunction::default(),
            add_contract_version: HostFunction::default(),
            disable_contract_version: HostFunction::default(),
            call_contract: HostFunction::new(
                DEFAULT_CALL_CONTRACT_COST,
                [
                    NOT_USED,
                    NOT_USED,
                    NOT_USED,
                    NOT_USED,
                    NOT_USED,
                    DEFAULT_CALL_CONTRACT_ARGS_SIZE_WEIGHT,
                    NOT_USED,
                ],
            ),
            call_versioned_contract: HostFunction::default(),
            get_named_arg_size: HostFunction::default(),
            get_named_arg: HostFunction::default(),
            remove_contract_user_group: HostFunction::default(),
            provision_contract_user_group_uref: HostFunction::default(),
            remove_contract_user_group_urefs: HostFunction::default(),
            print: HostFunction::new(
                DEFAULT_PRINT_COST,
                [NOT_USED, DEFAULT_PRINT_TEXT_SIZE_WEIGHT],
            ),
            blake2b: HostFunction::default(),
        }
    }
}

impl Distribution<HostFunctionCosts> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> HostFunctionCosts {
        HostFunctionCosts {
            read_value: rng.gen(),
            dictionary_get: rng.gen(),
            write: rng.gen(),
            dictionary_put: rng.gen(),
            add: rng.gen(),
            new_uref: rng.gen(),
            load_named_keys: rng.gen(),
            ret: rng.gen(),
            get_key: rng.gen(),
            has_key: rng.gen(),
            put_key: rng.gen(),
            remove_key: rng.gen(),
            revert: rng.gen(),
            is_valid_uref: rng.gen(),
            add_associated_key: rng.gen(),
            remove_associated_key: rng.gen(),
            update_associated_key: rng.gen(),
            set_action_threshold: rng.gen(),
            get_caller: rng.gen(),
            get_blocktime: rng.gen(),
            create_purse: rng.gen(),
            transfer_to_account: rng.gen(),
            transfer_from_purse_to_account: rng.gen(),
            transfer_from_purse_to_purse: rng.gen(),
            get_balance: rng.gen(),
            get_phase: rng.gen(),
            get_system_contract: rng.gen(),
            get_main_purse: rng.gen(),
            read_host_buffer: rng.gen(),
            create_contract_package_at_hash: rng.gen(),
            create_contract_user_group: rng.gen(),
            add_contract_version: rng.gen(),
            disable_contract_version: rng.gen(),
            call_contract: rng.gen(),
            call_versioned_contract: rng.gen(),
            get_named_arg_size: rng.gen(),
            get_named_arg: rng.gen(),
            remove_contract_user_group: rng.gen(),
            provision_contract_user_group_uref: rng.gen(),
            remove_contract_user_group_urefs: rng.gen(),
            print: rng.gen(),
            blake2b: rng.gen(),
        }
    }
}

#[doc(hidden)]
#[cfg(any(feature = "gens", test))]
pub mod gens {
    use proptest::prelude::*;

    use super::{Cost, HostFunction, HostFunctionCosts};

    pub fn host_function_cost_arb<T: Copy + Arbitrary>() -> impl Strategy<Value = HostFunction<T>> {
        (any::<Cost>(), any::<T>()).prop_map(|(cost, arguments)| HostFunction::new(cost, arguments))
    }

    prop_compose! {
        pub fn host_function_costs_arb() (
            read_value in host_function_cost_arb(),
            dictionary_get in host_function_cost_arb(),
            write in host_function_cost_arb(),
            dictionary_put in host_function_cost_arb(),
            add in host_function_cost_arb(),
            new_uref in host_function_cost_arb(),
            load_named_keys in host_function_cost_arb(),
            ret in host_function_cost_arb(),
            get_key in host_function_cost_arb(),
            has_key in host_function_cost_arb(),
            put_key in host_function_cost_arb(),
            remove_key in host_function_cost_arb(),
            revert in host_function_cost_arb(),
            is_valid_uref in host_function_cost_arb(),
            add_associated_key in host_function_cost_arb(),
            remove_associated_key in host_function_cost_arb(),
            update_associated_key in host_function_cost_arb(),
            set_action_threshold in host_function_cost_arb(),
            get_caller in host_function_cost_arb(),
            get_blocktime in host_function_cost_arb(),
            create_purse in host_function_cost_arb(),
            transfer_to_account in host_function_cost_arb(),
            transfer_from_purse_to_account in host_function_cost_arb(),
            transfer_from_purse_to_purse in host_function_cost_arb(),
            get_balance in host_function_cost_arb(),
            get_phase in host_function_cost_arb(),
            get_system_contract in host_function_cost_arb(),
            get_main_purse in host_function_cost_arb(),
            read_host_buffer in host_function_cost_arb(),
            create_contract_package_at_hash in host_function_cost_arb(),
            create_contract_user_group in host_function_cost_arb(),
            add_contract_version in host_function_cost_arb(),
            disable_contract_version in host_function_cost_arb(),
            call_contract in host_function_cost_arb(),
            call_versioned_contract in host_function_cost_arb(),
            get_named_arg_size in host_function_cost_arb(),
            get_named_arg in host_function_cost_arb(),
            remove_contract_user_group in host_function_cost_arb(),
            provision_contract_user_group_uref in host_function_cost_arb(),
            remove_contract_user_group_urefs in host_function_cost_arb(),
            print in host_function_cost_arb(),
            blake2b in host_function_cost_arb(),
        ) -> HostFunctionCosts {
            HostFunctionCosts {
                read_value,
                dictionary_get,
                write,
                dictionary_put,
                add,
                new_uref,
                load_named_keys,
                ret,
                get_key,
                has_key,
                put_key,
                remove_key,
                revert,
                is_valid_uref,
                add_associated_key,
                remove_associated_key,
                update_associated_key,
                set_action_threshold,
                get_caller,
                get_blocktime,
                create_purse,
                transfer_to_account,
                transfer_from_purse_to_account,
                transfer_from_purse_to_purse,
                get_balance,
                get_phase,
                get_system_contract,
                get_main_purse,
                read_host_buffer,
                create_contract_package_at_hash,
                create_contract_user_group,
                add_contract_version,
                disable_contract_version,
                call_contract,
                call_versioned_contract,
                get_named_arg_size,
                get_named_arg,
                remove_contract_user_group,
                provision_contract_user_group_uref,
                remove_contract_user_group_urefs,
                print,
                blake2b,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use casper_types::U512;

    use super::*;

    const COST: Cost = 42;
    const ARGUMENT_COSTS: [Cost; 3] = [123, 456, 789];
    const WEIGHTS: [Cost; 3] = [1000, 1100, 1200];

    #[test]
    fn calculate_gas_cost_for_host_function() {
        let host_function = HostFunction::new(COST, ARGUMENT_COSTS);
        let expected_cost = COST
            + (ARGUMENT_COSTS[0] * WEIGHTS[0])
            + (ARGUMENT_COSTS[1] * WEIGHTS[1])
            + (ARGUMENT_COSTS[2] * WEIGHTS[2]);
        assert_eq!(
            host_function.calculate_gas_cost(WEIGHTS),
            Gas::new(expected_cost.into())
        );
    }

    #[test]
    fn calculate_gas_cost_would_overflow() {
        let large_value = Cost::max_value();

        let host_function = HostFunction::new(
            large_value,
            [large_value, large_value, large_value, large_value],
        );

        let lhs =
            host_function.calculate_gas_cost([large_value, large_value, large_value, large_value]);

        let large_value = U512::from(large_value);
        let rhs = large_value + (U512::from(4) * large_value * large_value);

        assert_eq!(lhs, Gas::new(rhs));
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use casper_types::bytesrepr;

    use super::*;

    type Signature = [Cost; 10];

    proptest! {
        #[test]
        fn test_host_function(host_function in gens::host_function_cost_arb::<Signature>()) {
            bytesrepr::test_serialization_roundtrip(&host_function);
        }

        #[test]
        fn test_host_function_costs(host_function_costs in gens::host_function_costs_arb()) {
            bytesrepr::test_serialization_roundtrip(&host_function_costs);
        }
    }
}
