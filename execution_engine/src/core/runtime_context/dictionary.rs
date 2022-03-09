use borsh::{BorshDeserialize, BorshSerialize};
use casper_types::{bytesrepr::Bytes, CLType, CLTyped, CLValue, CLValueError, Key, StoredValue};

/// Wraps a [`CLValue`] for storage in a dictionary.
///
/// Note that we include the dictionary [`casper_types::URef`] and key used to create the
/// `Key::Dictionary` under which this value is stored.  This is to allow migration to a different
/// key representation in the future.
#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct DictionaryValue {
    /// Actual [`CLValue`] written to global state.
    cl_value: CLValue,
    /// [`URef`] seed bytes.
    seed_uref_addr: Bytes,
    /// Original key bytes.
    dictionary_item_key_bytes: Bytes,
}

impl DictionaryValue {
    pub fn new(
        cl_value: CLValue,
        seed_uref_addr: Vec<u8>,
        dictionary_item_key_bytes: Vec<u8>,
    ) -> Self {
        Self {
            cl_value,
            seed_uref_addr: seed_uref_addr.into(),
            dictionary_item_key_bytes: dictionary_item_key_bytes.into(),
        }
    }

    /// Get a reference to the [`DictionaryValue`]'s wrapper's cl value.
    pub fn into_cl_value(self) -> CLValue {
        self.cl_value
    }
}

impl CLTyped for DictionaryValue {
    fn cl_type() -> CLType {
        CLType::Any
    }
}

/// Inspects `key` argument whether it contains a dictionary variant, and checks if `stored_value`
/// contains a [`CLValue`], then it will attempt a conversion from the held clvalue into
/// [`DictionaryValue`] and returns the real [`CLValue`] held by it.
///
/// For any other combination of `key` and `stored_value` it returns its unmodified value.
pub fn handle_stored_value(
    key: Key,
    stored_value: StoredValue,
) -> Result<StoredValue, CLValueError> {
    match (key, stored_value) {
        (Key::Dictionary(_), StoredValue::CLValue(cl_value)) => {
            let wrapped_cl_value: DictionaryValue = cl_value.into_t()?;
            let cl_value = wrapped_cl_value.into_cl_value();
            Ok(StoredValue::CLValue(cl_value))
        }
        (_, stored_value) => Ok(stored_value),
    }
}

/// Wraps a [`StoredValue`] into [`DictionaryValue`] only if it contains a [`CLValue`] variant.
///
/// Used only for testing purposes.
#[cfg(test)]
pub fn handle_stored_value_into(
    key: Key,
    stored_value: StoredValue,
) -> Result<StoredValue, CLValueError> {
    match (key, stored_value) {
        (Key::Dictionary(_), StoredValue::CLValue(cl_value)) => {
            let wrapped_dictionary_value =
                DictionaryValue::new(cl_value, vec![0; 32], vec![255; 32]);
            let wrapped_cl_value = CLValue::from_t(wrapped_dictionary_value)?;
            Ok(StoredValue::CLValue(wrapped_cl_value))
        }
        (_, stored_value) => Ok(stored_value),
    }
}
