//! Capability traits: the only way core code touches the outside world.
//!
//! Core code is async but runtime-free — it returns futures; hosts drive them.
//! Every effect (clock, storage, later: sleep, random, http) is a small trait
//! here, wired per-target by the host (native: redb/tokio clock, wasm: IndexedDB/js Date).

use crate::async_trait;
use serde::{Deserialize, Serialize};

/// Unix timestamp in whole seconds. Domain code never touches std::time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(pub u64);

/// Monotonic-enough wall clock.
#[async_trait]
pub trait Clock: Clone + Send + Sync + 'static {
    async fn now(&self) -> Timestamp;
}

/// Minimal async key-value store. Values are JSON strings so every wire type
/// round-trips without per-store schema code.
#[async_trait]
pub trait Store: Clone + Send + Sync + 'static {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError>;
    async fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// Keys under `prefix` (lexicographic scan; fine for v1 scales).
    async fn list(&self, prefix: &str) -> Result<Vec<(String, String)>, StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store io: {0}")]
    Io(String),
}

/// Bundle of capabilities a host injects into service impls.
/// Apps extend this pattern with their own associated types per feature.
pub trait Caps: Clone + Send + Sync + 'static {
    type Clock: Clock;
    type Store: Store;
    fn clock(&self) -> &Self::Clock;
    fn store(&self) -> &Self::Store;
}
