//! Native capability implementations (host-side; std::time allowed HERE, never in core).
//!
//! # Effect home
//!
//! This crate is a *designated effect home*: the only place in a gabion app
//! where the banned effects (std::time, std::fs, std::env, std::net, threads)
//! are allowed to live. The ban in clippy.toml is global, so the opt-out has to
//! be explicit and crate-wide — and `gabion check` fails the build if a
//! `#![allow(clippy::disallowed_*)]` shows up in any crate that is not a
//! declared effect home. One line here, audited; not twelve scattered ones.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use gabion::async_trait;
use gabion::caps::{Caps, Clock, Random, Timestamp};
use gabion::testing::InMemoryStore;

pub mod filestore;
pub use filestore::FileStore;

/// OS entropy — the native impl of `Random`.
#[derive(Clone)]
pub struct SystemRandom;

#[async_trait]
impl Random for SystemRandom {
    async fn next_u64(&self) -> u64 {
        let mut buf = [0u8; 8];
        // No RNG is worse than a slow one for id uniqueness, and a host that
        // cannot read entropy has bigger problems than its id scheme.
        if getrandom::fill(&mut buf).is_err() {
            return std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0);
        }
        u64::from_le_bytes(buf)
    }
}

/// Wall clock from `std::time` — the native impl. Core code only sees `Clock`.
#[derive(Clone)]
pub struct SystemClock;

#[async_trait]
#[allow(clippy::unwrap_used)] // bootstrap-level: duration_since only errs on pre-epoch clocks
impl Clock for SystemClock {
    async fn now(&self) -> Timestamp {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Timestamp(secs)
    }
}

/// The native host's capability bundle.
#[derive(Clone)]
pub struct NativeCaps {
    clock: SystemClock,
    store: InMemoryStore,
    random: SystemRandom,
}

impl NativeCaps {
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: SystemClock,
            store: InMemoryStore::default(),
            random: SystemRandom,
        }
    }
}

impl Default for NativeCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl Caps for NativeCaps {
    type Clock = SystemClock;
    type Store = InMemoryStore;
    type Random = SystemRandom;
    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
    fn random(&self) -> &Self::Random {
        &self.random
    }
}

/// Persistent bundle for CLI hosts (state survives process exit).
#[derive(Clone)]
pub struct FileCaps {
    clock: SystemClock,
    store: FileStore,
    random: SystemRandom,
}

impl FileCaps {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Result<Self, gabion::caps::StoreError> {
        Ok(Self {
            clock: SystemClock,
            store: FileStore::new(path)?,
            random: SystemRandom,
        })
    }
}

impl Caps for FileCaps {
    type Clock = SystemClock;
    type Store = FileStore;
    type Random = SystemRandom;
    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
    fn random(&self) -> &Self::Random {
        &self.random
    }
}

// (StoreError stays at gabion::caps::StoreError — hosts use that path.)
