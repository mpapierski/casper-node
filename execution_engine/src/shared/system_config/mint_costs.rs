//! Costs of the mint system contract.
use casper_types::bytesrepr::{self, BorshDeserialize, BorshSerialize};
use datasize::DataSize;
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};

/// Default cost of the `mint` mint entry point.
pub const DEFAULT_MINT_COST: u32 = 2_500_000_000;
/// Default cost of the `reduce_total_supply` mint entry point.
pub const DEFAULT_REDUCE_TOTAL_SUPPLY_COST: u32 = 10_000;
/// Default cost of the `create` mint entry point.
pub const DEFAULT_CREATE_COST: u32 = 2_500_000_000;
/// Default cost of the `balance` mint entry point.
pub const DEFAULT_BALANCE_COST: u32 = 10_000;
/// Default cost of the `transfer` mint entry point.
pub const DEFAULT_TRANSFER_COST: u32 = 10_000;
/// Default cost of the `read_base_round_reward` mint entry point.
pub const DEFAULT_READ_BASE_ROUND_REWARD_COST: u32 = 10_000;

/// Description of the costs of calling mint entry points.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, DataSize)]
pub struct MintCosts {
    /// Cost of calling the `mint` entry point.
    pub mint: u32,
    /// Cost of calling the `reduce_total_supply` entry point.
    pub reduce_total_supply: u32,
    /// Cost of calling the `create` entry point.
    pub create: u32,
    /// Cost of calling the `balance` entry point.
    pub balance: u32,
    /// Cost of calling the `transfer` entry point.
    pub transfer: u32,
    /// Cost of calling the `read_base_round_reward` entry point.
    pub read_base_round_reward: u32,
}

impl Default for MintCosts {
    fn default() -> Self {
        Self {
            mint: DEFAULT_MINT_COST,
            reduce_total_supply: DEFAULT_REDUCE_TOTAL_SUPPLY_COST,
            create: DEFAULT_CREATE_COST,
            balance: DEFAULT_BALANCE_COST,
            transfer: DEFAULT_TRANSFER_COST,
            read_base_round_reward: DEFAULT_READ_BASE_ROUND_REWARD_COST,
        }
    }
}

impl Distribution<MintCosts> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> MintCosts {
        MintCosts {
            mint: rng.gen(),
            reduce_total_supply: rng.gen(),
            create: rng.gen(),
            balance: rng.gen(),
            transfer: rng.gen(),
            read_base_round_reward: rng.gen(),
        }
    }
}

#[doc(hidden)]
#[cfg(any(feature = "gens", test))]
pub mod gens {
    use proptest::{num, prop_compose};

    use super::MintCosts;

    prop_compose! {
        pub fn mint_costs_arb()(
            mint in num::u32::ANY,
            reduce_total_supply in num::u32::ANY,
            create in num::u32::ANY,
            balance in num::u32::ANY,
            transfer in num::u32::ANY,
            read_base_round_reward in num::u32::ANY,
        ) -> MintCosts {
            MintCosts {
                mint,
                reduce_total_supply,
                create,
                balance,
                transfer,
                read_base_round_reward,
            }
        }
    }
}
