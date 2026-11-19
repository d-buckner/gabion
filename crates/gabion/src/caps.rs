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

/// Source of unpredictability. Core code needs ids, and the two tempting
/// shortcuts are both wrong: reading the clock twice and hoping (two writes in
/// the same tick collide and overwrite silently), or `std::random`, which does
/// not exist on wasm. Hosts wire a real entropy source; tests wire a
/// deterministic one, which is what makes id-dependent code testable.
#[async_trait]
pub trait Random: Clone + Send + Sync + 'static {
    async fn next_u64(&self) -> u64;
}

/// Bundle of capabilities a host injects into service impls.
/// Apps extend this pattern with their own associated types per feature.
pub trait Caps: Clone + Send + Sync + 'static {
    type Clock: Clock;
    type Store: Store;
    type Random: Random;
    fn clock(&self) -> &Self::Clock;
    fn store(&self) -> &Self::Store;
    fn random(&self) -> &Self::Random;
}
