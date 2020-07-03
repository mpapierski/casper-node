use serde::{Deserialize, Serialize};

use crate::{
    ApiServerConfig, GossipTableConfig, SmallNetworkConfig, StorageConfig,
    ROOT_VALIDATOR_LISTENING_PORT,ContractRuntimeConfig,
};

/// Root configuration.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Network configuration for the validator-only network.
    pub validator_net: SmallNetworkConfig,
    /// Network configuration for the HTTP API.
    pub http_server: ApiServerConfig,
    /// On-disk storage configuration.
    pub storage: StorageConfig,
    /// Contract runtime configuration.
    pub contract_runtime: ContractRuntimeConfig,
    /// Gossip protocol configuration.
    pub gossip: GossipTableConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            validator_net: SmallNetworkConfig::default_on_port(ROOT_VALIDATOR_LISTENING_PORT),
            http_server: ApiServerConfig::default(),
            storage: StorageConfig::default(),
            contract_runtime: ContractRuntimeConfig::default(),
            gossip: GossipTableConfig::default(),
        }
    }
}
