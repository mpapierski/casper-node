//! Some newtypes.
mod macros;
use std::{
    fmt::{self, Display, Formatter},
    sync::{Arc, RwLock},
    time::Duration,
};

use bytes::Bytes;
use casper_types::ContractHash;
use serde::Serialize;
use uuid::Uuid;

/// Property assigned to correlation id.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Property {
    /// When we know exec path wouldn't reach wasm VM.
    Preprocess {
        original_bytes: Bytes,
    },
    Contract {
        contract_hash: ContractHash,
        original_bytes: Bytes,
    },
    WasmVM {
        original_bytes: Bytes,
        preprocess_duration: Duration,
        invoke_duration: Duration,
    },
    VMCacheHit {
        original_bytes: Bytes,
    },
    VMCacheMiss {
        original_bytes: Bytes,
    },
}

/// A correlation id is a unique identifier which can be used to track the progress of a given
/// execution engine operation.
#[derive(Clone, Debug)]
pub struct CorrelationId {
    uuid: Uuid,
    property_log: Arc<RwLock<Vec<Property>>>,
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self {
            uuid: Default::default(),
            property_log: Arc::new(Default::default()),
        }
    }
}

impl CorrelationId {
    /// Creates new unique `CorrelationId`.
    pub fn new() -> CorrelationId {
        CorrelationId {
            uuid: Uuid::new_v4(),
            property_log: Arc::new(Default::default()),
        }
    }

    /// Record a property
    pub fn record_property(&self, value: Property) {
        self.property_log.write().unwrap().push(value);
    }

    /// Get all properties
    pub fn trace(&self) -> Vec<Property> {
        self.property_log.read().unwrap().clone()
    }

    /// Returns true if the given unique identifier is empty.
    pub fn is_empty(&self) -> bool {
        self.uuid.is_nil()
    }
}

impl Display for CorrelationId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.uuid)
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{Hash, Hasher};

    use crate::shared::{newtypes::CorrelationId, utils};

    #[test]
    fn should_be_able_to_generate_correlation_id() {
        let correlation_id = CorrelationId::new();

        assert_ne!(
            correlation_id.to_string(),
            "00000000-0000-0000-0000-000000000000",
            "should not be empty value"
        )
    }

    #[test]
    fn should_support_to_string() {
        let correlation_id = CorrelationId::new();

        assert!(
            !correlation_id.is_empty(),
            "correlation_id should be produce string"
        )
    }

    #[test]
    fn should_support_to_string_no_type_encasement() {
        let correlation_id = CorrelationId::new();

        let correlation_id_string = correlation_id.to_string();

        assert!(
            !correlation_id_string.starts_with("CorrelationId"),
            "correlation_id should just be the inner value without tuple name"
        )
    }

    #[test]
    fn should_support_to_json() {
        let correlation_id = CorrelationId::new();

        let correlation_id_json = utils::jsonify(correlation_id.clone(), false);

        assert!(
            !correlation_id_json.is_empty(),
            "correlation_id should be produce json"
        )
    }

    #[test]
    fn should_support_is_display() {
        let correlation_id = CorrelationId::new();

        let display = format!("{}", correlation_id);

        assert!(!display.is_empty(), "display should not be empty")
    }

    #[test]
    fn should_support_is_empty() {
        let correlation_id = CorrelationId::new();

        assert!(
            !correlation_id.is_empty(),
            "correlation_id should not be empty"
        )
    }

    #[test]
    fn should_create_unique_id_on_new() {
        let correlation_id_lhs = CorrelationId::new();
        let correlation_id_rhs = CorrelationId::new();

        assert_ne!(
            correlation_id_lhs, correlation_id_rhs,
            "correlation_ids should be distinct"
        );
    }

    #[test]
    fn should_support_clone() {
        let correlation_id = CorrelationId::new();

        let cloned = correlation_id.clone();

        assert_eq!(correlation_id.clone(), cloned, "should be cloneable")
    }

    #[test]
    fn should_support_hash() {
        let correlation_id = CorrelationId::new();

        let mut state = std::collections::hash_map::DefaultHasher::new();

        correlation_id.hash(&mut state);

        let hash = state.finish();

        assert!(hash > 0, "should be hashable");
    }
}
