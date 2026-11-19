//! Deterministic capability impls for tests: fixed clock, in-memory store.
//! Core tests never touch real time or disk — that's what makes them pure.

use crate::async_trait;
use crate::caps::{Caps, Clock, Store, StoreError, Timestamp};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Clock frozen at a fixed instant (or stepping manually).
#[derive(Clone)]
pub struct MockClock(pub Timestamp);

#[async_trait]
impl Clock for MockClock {
    async fn now(&self) -> Timestamp {
        self.0
    }
}

/// In-memory Store; cheap and deterministic.
#[derive(Clone, Default)]
pub struct InMemoryStore {
    map: Arc<Mutex<BTreeMap<String, String>>>,
}

#[async_trait]
impl Store for InMemoryStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .map
            .lock()
            .map_err(|e| StoreError::Io(e.to_string()))?
            .get(key)
            .cloned())
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.map
            .lock()
            .map_err(|e| StoreError::Io(e.to_string()))?
            .insert(key.into(), value.into());
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.map
            .lock()
            .map_err(|e| StoreError::Io(e.to_string()))?
            .remove(key);
        Ok(())
    }
    async fn list(&self, prefix: &str) -> Result<Vec<(String, String)>, StoreError> {
        Ok(self
            .map
            .lock()
            .map_err(|e| StoreError::Io(e.to_string()))?
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

/// Test capability bundle.
#[derive(Clone)]
pub struct TestCaps {
    clock: MockClock,
    store: InMemoryStore,
}

impl TestCaps {
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: MockClock(Timestamp(1_700_000_000)),
            store: InMemoryStore::default(),
        }
    }
}

impl Default for TestCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl Caps for TestCaps {
    type Clock = MockClock;
    type Store = InMemoryStore;
    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
}
