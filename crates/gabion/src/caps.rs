//! Capability traits: the only way core code touches the outside world.
//!
//! Core code is async but runtime-free — it returns futures; hosts drive them.
//! Every effect (clock, storage, sleep, randomness, http) is a small trait here,
//! wired per-target by the host (native: tokio timer, reqwest client, file
//! store; wasm: fetch, IndexedDB, js Date).

use crate::async_trait;
// Re-exported so a core names every capability verb from one module.
pub use crate::channel::Channel;
pub use crate::frames::StreamError;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

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

    /// A write that carries the caller, for a store that wants to record who made
    /// it.
    ///
    /// The caller reaches every service method as `Ctx`; this is the one effect
    /// that usually wants to keep it, because "who wrote this row" is the first
    /// question an audit asks. The default drops it and delegates to
    /// [`Store::set`], so an implementation that does not care compiles unchanged —
    /// and one that does can answer [`written_by`](crate::testing::InMemoryStore)
    /// or its own equivalent.
    async fn set_as(
        &self,
        _ctx: &crate::ctx::Ctx,
        key: &str,
        value: &str,
    ) -> Result<(), StoreError> {
        self.set(key, value).await
    }
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

/// A boxed timer.
///
/// [`Sleep`] returns these instead of an `async fn` so the future can outlive
/// the call that made it. That is what lets [`timeout`] hold one.
pub type Timer = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// The ability to wait, and to bound a wait.
///
/// Core code never names a runtime: `std::thread::sleep` blocks a worker and
/// does not exist on wasm. A host wires a real timer. A test wires a virtual one
/// whose clock moves only when code asks to wait, so retry and debounce logic
/// runs at full speed and stays deterministic.
pub trait Sleep: Clone + Send + Sync + 'static {
    /// Wait for `millis` of this capability's clock.
    fn sleep(&self, millis: u64) -> Timer;

    /// A future that completes once `millis` of this clock has passed.
    ///
    /// Separate from [`Sleep::sleep`] because a deadline must not move the clock
    /// it measures: a timer that advances a virtual clock would fire every
    /// timeout the moment it was polled.
    fn timer(&self, millis: u64) -> Timer;
}

/// The error from [`timeout`] when the deadline arrives first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("timed out")]
pub struct TimedOut;

/// Race `fut` against a deadline on `sleep`'s clock.
///
/// Under a real timer the call is cut off at the deadline. Under the test clock
/// virtual time moves only when code asks to wait, so a future that never sleeps
/// is never interrupted. That is the same rule tokio's virtual time follows, and
/// it is what keeps a timeout test deterministic instead of wall-clock dependent.
#[must_use = "a timeout does nothing until it is polled"]
pub fn timeout<S: Sleep, T>(
    sleep: &S,
    millis: u64,
    fut: Pin<Box<dyn Future<Output = T> + Send>>,
) -> Timeout<T> {
    Timeout {
        timer: sleep.timer(millis),
        inner: fut,
    }
}

/// The future returned by [`timeout`].
#[must_use = "a timeout does nothing until it is polled"]
pub struct Timeout<T> {
    timer: Timer,
    inner: Pin<Box<dyn Future<Output = T> + Send>>,
}

impl<T> Future for Timeout<T> {
    type Output = Result<T, TimedOut>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Poll::Ready(value) = this.inner.as_mut().poll(cx) {
            return Poll::Ready(Ok(value));
        }
        if this.timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(TimedOut));
        }
        Poll::Pending
    }
}

/// The verbs a core may ask for. v1 has no use for the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
        }
    }
}

/// A call to another service. Core code names a URL and a body; it never names
/// an HTTP library, so one core runs behind a native client and behind a wasm
/// `fetch` client without a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[must_use]
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: Vec::new(),
            body,
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// The reply, flat. A status outside 2xx is a response, not a transport error:
/// the caller decides which [`ErrorKind`](crate::error::ErrorKind) it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request never became a response: DNS, TLS, connection refused, a
    /// client-side deadline. A 404 is not this.
    #[error("http transport: {0}")]
    Transport(String),
}

/// A core's way to another service. This is the capability the feature wall
/// needs: feature A reaches feature B through B's `*-api` contract, and the
/// generated HTTP client for that contract (still planned) travels on `Http`
/// rather than on an HTTP crate in the core.
#[async_trait]
pub trait Http: Clone + Send + Sync + 'static {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// The ability to open a full-duplex channel and stream over it.
///
/// `Http` cannot carry a stream: [`HttpResponse`] is a whole body in memory, and
/// the generated client has to implement the whole service trait, so a stream op
/// needs a verb of its own (`docs/streaming.md` §6). The channel is one WebSocket;
/// which op it carries is in the [`Frame`](crate::frames::Frame)s written to it,
/// not in the URL.
pub trait Streams: Clone + Send + Sync + 'static {
    /// Open the channel `request` names and hand back both directions.
    ///
    /// `request.url` is the endpoint — `ws://host/api/ws` on native, the same
    /// address the HTTP calls go to. Opening is not the same as streaming: no
    /// stream exists until an `open` frame is written on the channel.
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError>;
}

/// The `Streams` capability of a process that has no socket: it refuses, by name.
///
/// A generated client builds one of these when its host gave it only `Http`, so a
/// stream call fails with the reason instead of the client failing to compile.
/// The alternative — a client generic only over `Http` and a stream method that
/// cannot exist — would make one listen op unbuildable for every wasm host.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoStreams;

impl Streams for NoStreams {
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError> {
        Err(StreamError::Transport(format!(
            "this client has no stream transport; build it with `with_streams` to call `{}`",
            request.url
        )))
    }
}

/// Bundle of capabilities a host injects into service impls.
/// Apps extend this pattern with their own associated types per feature.
pub trait Caps: Clone + Send + Sync + 'static {
    type Clock: Clock;
    type Store: Store;
    type Random: Random;
    type Sleep: Sleep;
    type Http: Http;
    type Streams: Streams;
    fn clock(&self) -> &Self::Clock;
    fn store(&self) -> &Self::Store;
    fn random(&self) -> &Self::Random;
    fn sleep(&self) -> &Self::Sleep;
    fn http(&self) -> &Self::Http;
    fn streams(&self) -> &Self::Streams;
}
