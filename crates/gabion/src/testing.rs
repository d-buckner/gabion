//! Deterministic capability impls for tests: virtual clock, in-memory store,
//! scripted http. Core tests never touch real time or disk — that is what makes
//! them pure.

use crate::async_trait;
use crate::caps::{
    Caps, Clock, Http, HttpError, HttpMethod, HttpRequest, HttpResponse, Random, Sleep, Store,
    StoreError, Timer, Timestamp,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// A clock a test controls. It moves when [`MockTime::advance`] runs or when a
/// [`MockSleep`] waits, and otherwise not at all.
#[derive(Clone, Default)]
pub struct MockTime {
    inner: Arc<Mutex<Virtual>>,
}

#[derive(Default)]
struct Virtual {
    now_ms: u64,
    /// Timers parked until the clock reaches their deadline.
    waiters: Vec<Waker>,
}

impl MockTime {
    #[must_use]
    pub fn new(start_ms: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Virtual {
                now_ms: start_ms,
                waiters: Vec::new(),
            })),
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.lock().now_ms
    }

    /// Move the clock forward and wake every timer waiting on it.
    pub fn advance(&self, millis: u64) {
        let wakers: Vec<Waker> = {
            let mut v = self.lock();
            v.now_ms = v.now_ms.saturating_add(millis);
            std::mem::take(&mut v.waiters)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Park the current task until the clock moves. Called by a timer that found
    /// the deadline still in the future.
    fn park(&self, waker: &Waker) {
        let mut v = self.lock();
        if v.waiters.iter().any(|w| w.will_wake(waker)) {
            return;
        }
        v.waiters.push(waker.clone());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Virtual> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Clock over a [`MockTime`]. It reads whole seconds, like every other `Clock`.
#[derive(Clone)]
pub struct MockClock {
    time: MockTime,
}

impl MockClock {
    #[must_use]
    pub fn new(start: Timestamp) -> Self {
        Self {
            time: MockTime::new(start.0.saturating_mul(1_000)),
        }
    }

    /// The virtual clock behind this clock, so a test can advance time directly.
    #[must_use]
    pub fn time(&self) -> &MockTime {
        &self.time
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new(Timestamp(0))
    }
}

#[async_trait]
impl Clock for MockClock {
    async fn now(&self) -> Timestamp {
        Timestamp(self.time.now_ms() / 1_000)
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

/// Deterministic randomness: a counter through a mixing function, so ids look
/// scattered but a test run is reproducible bit for bit.
#[derive(Clone, Default)]
pub struct MockRandom {
    state: Arc<Mutex<u64>>,
}

impl MockRandom {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(seed)),
        }
    }
}

#[async_trait]
impl Random for MockRandom {
    async fn next_u64(&self) -> u64 {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // splitmix64 step: cheap, well-mixed, and identical on every target.
        *guard = guard.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *guard;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Waiting at no wall-clock cost: `sleep` moves the virtual clock and yields
/// once, so a [`timeout`](crate::caps::timeout) racing it sees the deadline pass.
#[derive(Clone)]
pub struct MockSleep {
    time: MockTime,
}

impl MockSleep {
    #[must_use]
    pub fn new(time: MockTime) -> Self {
        Self { time }
    }
}

impl Sleep for MockSleep {
    fn sleep(&self, millis: u64) -> Timer {
        let time = self.time.clone();
        Box::pin(async move {
            time.advance(millis);
            YieldOnce::default().await;
        })
    }

    fn timer(&self, millis: u64) -> Timer {
        let deadline = self.time.now_ms().saturating_add(millis);
        Box::pin(VirtualTimer {
            time: self.time.clone(),
            deadline,
        })
    }
}

/// Yields once, then completes. It is what gives a racing deadline a chance to
/// be polled while the waiter is still in flight.
#[derive(Default)]
struct YieldOnce {
    polled: bool,
}

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.polled {
            return Poll::Ready(());
        }
        self.polled = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Ready once the virtual clock has reached `deadline`.
struct VirtualTimer {
    time: MockTime,
    deadline: u64,
}

impl Future for VirtualTimer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.time.now_ms() >= this.deadline {
            return Poll::Ready(());
        }
        this.time.park(cx.waker());
        // Re-check after registering: the clock may have moved between the first
        // read and the registration, and a lost wake would hang the test.
        if this.time.now_ms() >= this.deadline {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

/// Scripted http: a test says what each call returns and reads back what was
/// sent. A call nobody scripted is an error, not an empty 200.
#[derive(Clone, Default)]
pub struct MockHttp {
    routes: Arc<Mutex<BTreeMap<String, HttpResponse>>>,
    sent: Arc<Mutex<Vec<HttpRequest>>>,
}

impl MockHttp {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the reply for `METHOD url`, for example
    /// `POST http://todo.internal/api/todo.create`.
    #[must_use]
    pub fn route(self, method: HttpMethod, url: &str, reply: HttpResponse) -> Self {
        self.routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key(method.as_str(), url), reply);
        self
    }

    /// Every request the capability was asked to send, in order.
    #[must_use]
    pub fn sent(&self) -> Vec<HttpRequest> {
        self.sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, request: &HttpRequest) {
        self.sent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.clone());
    }

    fn reply(&self, request: &HttpRequest) -> Option<HttpResponse> {
        self.routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key(request.method.as_str(), &request.url))
            .cloned()
    }
}

fn key(method: &str, url: &str) -> String {
    format!("{method} {url}")
}

#[async_trait]
impl Http for MockHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.record(&request);
        self.reply(&request).ok_or_else(|| {
            HttpError::Transport(format!(
                "no scripted response for {} {}",
                request.method.as_str(),
                request.url
            ))
        })
    }
}

/// Test capability bundle.
#[derive(Clone)]
pub struct TestCaps {
    clock: MockClock,
    store: InMemoryStore,
    random: MockRandom,
    sleep: MockSleep,
    http: MockHttp,
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
}
