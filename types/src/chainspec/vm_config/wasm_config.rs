//! Configuration of the Wasm execution engine.
#[cfg(feature = "datasize")]
use datasize::DataSize;
#[cfg(any(feature = "testing", test))]
use rand::{
    distr::{Distribution, StandardUniform},
    Rng,
};
use serde::{Deserialize, Serialize};

use crate::{
    bytesrepr::{self, FromBytes, ToBytes},
    chainspec::vm_config::MessageLimits,
};

use super::{wasm_v1_config::WasmV1Config, wasm_v2_config::WasmV2Config};

/// Configuration of the Wasm execution environment.
///
/// This structure contains various Wasm execution configuration options, such as memory limits,
/// stack limits and costs.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[cfg_attr(feature = "datasize", derive(DataSize))]
#[serde(deny_unknown_fields)]
pub struct WasmConfig {
    /// Messages limits.
    messages_limits: MessageLimits,
    /// Configuration for wasms in v1 execution engine.
    v1: WasmV1Config,
    /// Configuration for wasms in v2 execution engine.
    v2: WasmV2Config,
}

impl WasmConfig {
    /// Creates new Wasm config.
    pub const fn new(messages_limits: MessageLimits, v1: WasmV1Config, v2: WasmV2Config) -> Self {
        Self {
            messages_limits,
            v1,
            v2,
        }
    }

    /// Returns the limits config for messages.
    pub fn messages_limits(&self) -> MessageLimits {
        self.messages_limits
    }

    /// Returns the config for v1 wasms.
    pub fn v1(&self) -> &WasmV1Config {
        &self.v1
    }

    /// Returns mutable v1 reference
    pub fn v1_mut(&mut self) -> &mut WasmV1Config {
        &mut self.v1
    }

    /// Returns the config for v2 wasms.
    pub fn v2(&self) -> &WasmV2Config {
        &self.v2
    }

    /// Returns mutable v2 reference
    pub fn v2_mut(&mut self) -> &mut WasmV2Config {
        &mut self.v2
    }
}

impl ToBytes for WasmConfig {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        let mut ret = bytesrepr::unchecked_allocate_buffer(self);
        ret.append(&mut self.messages_limits.to_bytes()?);
        ret.append(&mut self.v1.to_bytes()?);
        ret.append(&mut self.v2.to_bytes()?);
        Ok(ret)
    }

    fn serialized_length(&self) -> usize {
        self.messages_limits.serialized_length()
            + self.v1.serialized_length()
            + self.v2.serialized_length()
    }
}

impl FromBytes for WasmConfig {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (messages_limits, rem) = FromBytes::from_bytes(bytes)?;
        let (v1, rem) = FromBytes::from_bytes(rem)?;
        let (v2, rem) = FromBytes::from_bytes(rem)?;

        Ok((
            WasmConfig {
                messages_limits,
                v1,
                v2,
            },
            rem,
        ))
    }
}

#[cfg(any(feature = "testing", test))]
impl Distribution<WasmConfig> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> WasmConfig {
        WasmConfig {
            messages_limits: rng.gen(),
            v1: rng.gen(),
            v2: rng.gen(),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use proptest::prelude::*;
    proptest! {
        #[test]
        fn bytesrepr_roundtrip(wasm_config in super::gens::wasm_config_arb()) {
            bytesrepr::test_serialization_roundtrip(&wasm_config);
        }
    }
}

#[doc(hidden)]
#[cfg(any(feature = "gens", test))]
pub mod gens {
    use proptest::prop_compose;

    use crate::{
        chainspec::vm_config::{
            message_limits::gens::message_limits_arb, wasm_v1_config::gens::wasm_v1_config_arb,
            wasm_v2_config::gens::wasm_v2_config_arb,
        },
        WasmConfig,
    };

    prop_compose! {
        pub fn wasm_config_arb() (
            v1 in wasm_v1_config_arb(),
            v2 in wasm_v2_config_arb(),
            messages_limits in message_limits_arb(),
        ) -> WasmConfig {
            WasmConfig {
                messages_limits,
                v1,
                v2
            }
        }
    }
}
