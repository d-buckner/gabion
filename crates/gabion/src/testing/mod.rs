//! Deterministic capability impls for tests: virtual clock, in-memory store,
//! scripted http. Core tests never touch real time or disk — that is what makes
//! them pure.

use crate::caps::{Caps, Timestamp};

mod mocks;
mod streams;
pub use mocks::{InMemoryStore, MockClock, MockHttp, MockRandom, MockSleep, MockTime};
pub use streams::{MockStreams, Script, Tail};

/// Test capability bundle.
#[derive(Clone)]
pub struct TestCaps {
    clock: MockClock,
    store: InMemoryStore,
    random: MockRandom,
    sleep: MockSleep,
    http: MockHttp,
    streams: MockStreams,
}

impl TestCaps {
    #[must_use]
    pub fn new() -> Self {
        let clock = MockClock::new(Timestamp(1_700_000_000));
        Self {
            store: InMemoryStore::default(),
            random: MockRandom::new(0x5EED),
            sleep: MockSleep::new(clock.time().clone()),
            http: MockHttp::default(),
            streams: MockStreams::default(),
            clock,
        }
    }

    /// The virtual clock, so a test can advance time without a `Sleep` handle.
    #[must_use]
    pub fn time(&self) -> &MockTime {
        self.clock.time()
    }

    /// Swap in a scripted `http`, so a core that calls out is testable without a
    /// socket: `TestCaps::new().with_http(MockHttp::new().route(..))`.
    #[must_use]
    pub fn with_http(mut self, http: MockHttp) -> Self {
        self.http = http;
        self
    }

    /// The http capability, so a test can read back what the core asked to send.
    #[must_use]
    pub fn http(&self) -> &MockHttp {
        &self.http
    }

    /// Swap in a scripted `streams`, so a core that subscribes is testable
    /// without a socket: `TestCaps::new().with_streams(MockStreams::new().script(..))`.
    #[must_use]
    pub fn with_streams(mut self, streams: MockStreams) -> Self {
        self.streams = streams;
        self
    }

    /// The stream capability, so a test can read back what the core opened.
    #[must_use]
    pub fn streams(&self) -> &MockStreams {
        &self.streams
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
    type Random = MockRandom;
    type Sleep = MockSleep;
    type Http = MockHttp;
    type Streams = MockStreams;
    fn clock(&self) -> &Self::Clock {
        &self.clock
    }
    fn store(&self) -> &Self::Store {
        &self.store
    }
    fn random(&self) -> &Self::Random {
        &self.random
    }
    fn sleep(&self) -> &Self::Sleep {
        &self.sleep
    }
    fn http(&self) -> &Self::Http {
        &self.http
    }
    fn streams(&self) -> &Self::Streams {
        &self.streams
    }
}
