use std::{
    collections::HashMap,
    mem,
    ops::Deref,
    sync::{Arc, RwLock},
};

use tracing::error;

use casper_hashing::Digest;
use casper_types::{Key, StoredValue};

use crate::{
    shared::{additive_map::AdditiveMap, newtypes::CorrelationId, transform::Transform},
    storage::{
        error,
        global_state::{CommitError, CommitProvider, StateProvider, StateReader},
        store::Store,
        transaction_source::{lmdb::LmdbEnvironment, Transaction, TransactionSource},
        trie::{merkle_proof::TrieMerkleProof, Trie},
        trie_store::{
            lmdb::LmdbTrieStore,
            operations::{
                keys_with_prefix, missing_trie_keys, put_trie, read, read_with_proof, ReadResult,
            },
        },
    },
};

type SharedCache = Arc<RwLock<Cache>>;

struct Cache {
    stored_values: HashMap<Key, StoredValue>,
}

impl Cache {
    fn new() -> Self {
        Cache {
            stored_values: HashMap::new(),
        }
    }

    fn insert(&mut self, key: Key, value: StoredValue) {
        self.stored_values.insert(key, value);
    }

    fn get(&self, key: &Key) -> Option<&StoredValue> {
        self.stored_values.get(key)
    }

    fn into_inner(self) -> HashMap<Key, StoredValue> {
        self.stored_values
    }
}

/// Global state implemented against LMDB as a backing data store.
pub struct ScratchGlobalState {
    /// Underlying, cached stored values.
    cache: SharedCache,
    /// Environment for LMDB.
    pub(crate) environment: Arc<LmdbEnvironment>,
    /// Trie store held within LMDB.
    pub(crate) trie_store: Arc<LmdbTrieStore>,
    // TODO: make this a lazy-static
    /// Empty root hash used for a new trie.
    pub(crate) empty_root_hash: Digest,
}

/// Represents a "view" of global state at a particular root hash.
pub struct ScratchGlobalStateView {
    cache: SharedCache,
    /// Environment for LMDB.
    pub(crate) environment: Arc<LmdbEnvironment>,
    /// Trie store held within LMDB.
    pub(crate) store: Arc<LmdbTrieStore>,
    /// Root hash of this "view".
    pub(crate) root_hash: Digest,
}

impl ScratchGlobalState {
    /// Creates a state from an existing environment, store, and root_hash.
    /// Intended to be used for testing.
    pub fn new(
        environment: Arc<LmdbEnvironment>,
        trie_store: Arc<LmdbTrieStore>,
        empty_root_hash: Digest,
    ) -> Self {
        ScratchGlobalState {
            cache: Arc::new(RwLock::new(Cache::new())),
            environment,
            trie_store,
            empty_root_hash,
        }
    }

    /// Consume self and return inner cache.
    pub fn into_inner(self) -> HashMap<Key, StoredValue> {
        let cache = mem::replace(&mut *self.cache.write().unwrap(), Cache::new());
        cache.into_inner()
    }
}

impl StateReader<Key, StoredValue> for ScratchGlobalStateView {
    type Error = error::Error;

    fn read(
        &self,
        correlation_id: CorrelationId,
        key: &Key,
    ) -> Result<Option<StoredValue>, Self::Error> {
        if let Some(value) = self.cache.read().unwrap().get(key) {
            return Ok(Some(value.clone()));
        }
        let txn = self.environment.create_read_txn()?;
        let ret = match read::<Key, StoredValue, lmdb::RoTransaction, LmdbTrieStore, Self::Error>(
            correlation_id,
            &txn,
            self.store.deref(),
            &self.root_hash,
            key,
        )? {
            ReadResult::Found(value) => {
                self.cache.write().unwrap().insert(*key, value.clone());
                Some(value)
            }
            ReadResult::NotFound => None,
            ReadResult::RootNotFound => panic!("ScratchGlobalState has invalid root"),
        };
        txn.commit()?;
        Ok(ret)
    }

    fn read_with_proof(
        &self,
        correlation_id: CorrelationId,
        key: &Key,
    ) -> Result<Option<TrieMerkleProof<Key, StoredValue>>, Self::Error> {
        let txn = self.environment.create_read_txn()?;
        let ret = match read_with_proof::<
            Key,
            StoredValue,
            lmdb::RoTransaction,
            LmdbTrieStore,
            Self::Error,
        >(
            correlation_id,
            &txn,
            self.store.deref(),
            &self.root_hash,
            key,
        )? {
            ReadResult::Found(value) => Some(value),
            ReadResult::NotFound => None,
            ReadResult::RootNotFound => panic!("LmdbWithCacheGlobalState has invalid root"),
        };
        txn.commit()?;
        Ok(ret)
    }

    fn keys_with_prefix(
        &self,
        correlation_id: CorrelationId,
        prefix: &[u8],
    ) -> Result<Vec<Key>, Self::Error> {
        let txn = self.environment.create_read_txn()?;
        let keys_iter = keys_with_prefix::<Key, StoredValue, _, _>(
            correlation_id,
            &txn,
            self.store.deref(),
            &self.root_hash,
            prefix,
        );
        let mut ret = Vec::new();
        for result in keys_iter {
            match result {
                Ok(key) => ret.push(key),
                Err(error) => return Err(error),
            }
        }
        txn.commit()?;
        Ok(ret)
    }
}

impl CommitProvider for ScratchGlobalState {
    /// State hash returned is the one provided, as we do not write to lmdb with this kind of global
    /// state.
    fn commit(
        &self,
        _correlation_id: CorrelationId,
        state_hash: Digest,
        effects: AdditiveMap<Key, Transform>,
    ) -> Result<Digest, Self::Error> {
        for (key, transform) in effects.into_iter() {
            let cached_value = self.cache.read().unwrap().get(&key).cloned();
            let value = match (cached_value, transform) {
                (None, Transform::Write(new_value)) => new_value,
                (None, transform) => {
                    error!(
                        ?key,
                        ?transform,
                        "Key not found while attempting to apply transform"
                    );
                    return Err(CommitError::KeyNotFound(key).into());
                }
                (Some(current_value), transform) => match transform.apply(current_value.clone()) {
                    Ok(updated_value) => updated_value,
                    Err(err) => {
                        error!(?key, ?err, "Key found, but could not apply transform");
                        return Err(CommitError::TransformError(err).into());
                    }
                },
            };

            self.cache.write().unwrap().insert(key, value);
        }
        Ok(state_hash)
    }
}

impl StateProvider for ScratchGlobalState {
    type Error = error::Error;

    type Reader = ScratchGlobalStateView;

    fn checkout(&self, state_hash: Digest) -> Result<Option<Self::Reader>, Self::Error> {
        let txn = self.environment.create_read_txn()?;
        let maybe_root: Option<Trie<Key, StoredValue>> = self.trie_store.get(&txn, &state_hash)?;
        let maybe_state = maybe_root.map(|_| ScratchGlobalStateView {
            cache: Arc::clone(&self.cache),
            environment: Arc::clone(&self.environment),
            store: Arc::clone(&self.trie_store),
            root_hash: state_hash,
        });
        txn.commit()?;
        Ok(maybe_state)
    }

    fn empty_root(&self) -> Digest {
        self.empty_root_hash
    }

    fn get_trie(
        &self,
        _correlation_id: CorrelationId,
        trie_key: &Digest,
    ) -> Result<Option<Trie<Key, StoredValue>>, Self::Error> {
        let txn = self.environment.create_read_txn()?;
        let ret: Option<Trie<Key, StoredValue>> = self.trie_store.get(&txn, trie_key)?;
        txn.commit()?;
        Ok(ret)
    }

    fn put_trie(
        &self,
        correlation_id: CorrelationId,
        trie: &Trie<Key, StoredValue>,
    ) -> Result<Digest, Self::Error> {
        let mut txn = self.environment.create_read_write_txn()?;
        let trie_hash = put_trie::<
            Key,
            StoredValue,
            lmdb::RwTransaction,
            LmdbTrieStore,
            Self::Error,
        >(correlation_id, &mut txn, &self.trie_store, trie)?;
        txn.commit()?;
        Ok(trie_hash)
    }

    /// Finds all of the keys of missing descendant `Trie<K,V>` values
    fn missing_trie_keys(
        &self,
        correlation_id: CorrelationId,
        trie_keys: Vec<Digest>,
    ) -> Result<Vec<Digest>, Self::Error> {
        let txn = self.environment.create_read_txn()?;
        let missing_descendants =
            missing_trie_keys::<Key, StoredValue, lmdb::RoTransaction, LmdbTrieStore, Self::Error>(
                correlation_id,
                &txn,
                self.trie_store.deref(),
                trie_keys,
            )?;
        txn.commit()?;
        Ok(missing_descendants)
    }
}

#[cfg(test)]
mod tests {

    use lmdb::DatabaseFlags;
    use tempfile::tempdir;

    use casper_hashing::Digest;
    use casper_types::{account::AccountHash, CLValue};

    use super::*;
    use crate::storage::{
        global_state::{lmdb::LmdbGlobalState, CommitProvider},
        trie_store::operations::{write, WriteResult},
        DEFAULT_TEST_MAX_DB_SIZE, DEFAULT_TEST_MAX_READERS,
    };

    #[derive(Debug, Clone)]
    struct TestPair {
        key: Key,
        value: StoredValue,
    }

    fn create_test_pairs() -> [TestPair; 2] {
        [
            TestPair {
                key: Key::Account(AccountHash::new([1_u8; 32])),
                value: StoredValue::CLValue(CLValue::from_t(1_i32).unwrap()),
            },
            TestPair {
                key: Key::Account(AccountHash::new([2_u8; 32])),
                value: StoredValue::CLValue(CLValue::from_t(2_i32).unwrap()),
            },
        ]
    }

    fn create_test_pairs_updated() -> [TestPair; 3] {
        [
            TestPair {
                key: Key::Account(AccountHash::new([1u8; 32])),
                value: StoredValue::CLValue(CLValue::from_t("one".to_string()).unwrap()),
            },
            TestPair {
                key: Key::Account(AccountHash::new([2u8; 32])),
                value: StoredValue::CLValue(CLValue::from_t("two".to_string()).unwrap()),
            },
            TestPair {
                key: Key::Account(AccountHash::new([3u8; 32])),
                value: StoredValue::CLValue(CLValue::from_t(3_i32).unwrap()),
            },
        ]
    }

    struct TestState {
        state: LmdbGlobalState,
        root_hash: Digest,
    }

    fn create_test_state() -> TestState {
        let correlation_id = CorrelationId::new();
        let temp_dir = tempdir().unwrap();
        let environment = Arc::new(
            LmdbEnvironment::new(
                &temp_dir.path().to_path_buf(),
                DEFAULT_TEST_MAX_DB_SIZE,
                DEFAULT_TEST_MAX_READERS,
                true,
            )
            .unwrap(),
        );
        let trie_store =
            Arc::new(LmdbTrieStore::new(&environment, None, DatabaseFlags::empty()).unwrap());

        let state = LmdbGlobalState::empty(environment, trie_store).unwrap();
        let mut current_root = state.empty_root_hash;
        {
            let mut txn = state.environment.create_read_write_txn().unwrap();

            for TestPair { key, value } in &create_test_pairs() {
                match write::<_, _, _, LmdbTrieStore, error::Error>(
                    correlation_id,
                    &mut txn,
                    &state.trie_store,
                    &current_root,
                    key,
                    value,
                )
                .unwrap()
                {
                    WriteResult::Written(root_hash) => {
                        current_root = root_hash;
                    }
                    WriteResult::AlreadyExists => (),
                    WriteResult::RootNotFound => {
                        panic!("LmdbWithCacheGlobalState has invalid root")
                    }
                }
            }

            txn.commit().unwrap();
        }
        TestState {
            state,
            root_hash: current_root,
        }
    }

    #[test]
    fn checkout_fails_if_unknown_hash_is_given() {
        let TestState { state, .. } = create_test_state();
        let fake_hash: Digest = Digest::hash(&[1u8; 32]);
        let result = state.checkout(fake_hash).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn commit_updates_state() {
        let correlation_id = CorrelationId::new();
        let test_pairs_updated = create_test_pairs_updated();

        let TestState { state, root_hash } = create_test_state();

        let scratch = state.create_scratch();

        let effects: AdditiveMap<Key, Transform> = {
            let mut tmp = AdditiveMap::new();
            for TestPair { key, value } in &test_pairs_updated {
                tmp.insert(*key, Transform::Write(value.to_owned()));
            }
            tmp
        };

        scratch
            .commit(correlation_id, root_hash, effects.clone())
            .unwrap();

        let updated_hash = state.commit(correlation_id, root_hash, effects).unwrap();
        let stored_values = scratch.into_inner();
        let updated_checkout = state.checkout(updated_hash).unwrap().unwrap();
        let all_keys = updated_checkout
            .keys_with_prefix(correlation_id, &[])
            .unwrap();

        assert_eq!(all_keys.len(), stored_values.len());

        for key in all_keys {
            println!("key {}", key);
            assert!(stored_values.get(&key).is_some());
            assert_eq!(
                stored_values.get(&key),
                updated_checkout
                    .read(correlation_id, &key)
                    .unwrap()
                    .as_ref()
            );
        }

        for TestPair { key, value } in test_pairs_updated.iter().cloned() {
            assert_eq!(
                Some(value),
                updated_checkout.read(correlation_id, &key).unwrap()
            );
        }
    }

    #[test]
    fn commit_updates_state_and_original_state_stays_intact() {
        let correlation_id = CorrelationId::new();
        let test_pairs_updated = create_test_pairs_updated();

        let TestState {
            state, root_hash, ..
        } = create_test_state();

        let effects: AdditiveMap<Key, Transform> = {
            let mut tmp = AdditiveMap::new();
            for TestPair { key, value } in &test_pairs_updated {
                tmp.insert(*key, Transform::Write(value.to_owned()));
            }
            tmp
        };

        let updated_hash = state.commit(correlation_id, root_hash, effects).unwrap();

        let updated_checkout = state.checkout(updated_hash).unwrap().unwrap();
        for TestPair { key, value } in test_pairs_updated.iter().cloned() {
            assert_eq!(
                Some(value),
                updated_checkout.read(correlation_id, &key).unwrap()
            );
        }

        let original_checkout = state.checkout(root_hash).unwrap().unwrap();
        for TestPair { key, value } in create_test_pairs().iter().cloned() {
            assert_eq!(
                Some(value),
                original_checkout.read(correlation_id, &key).unwrap()
            );
        }
        assert_eq!(
            None,
            original_checkout
                .read(correlation_id, &test_pairs_updated[2].key)
                .unwrap()
        );
    }
}
