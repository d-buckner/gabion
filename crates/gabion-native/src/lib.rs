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
use gabion::caps::{
    Caps, Clock, Http, HttpError, HttpMethod, HttpRequest, HttpResponse, Random, Sleep, Timer,
    Timestamp,
};
use gabion::testing::InMemoryStore;
use std::time::Duration;

pub mod filestore;
pub use filestore::FileStore;
/// The stream transport: one WebSocket per host, written as frames.
pub mod streams;
pub use streams::WsStreams;

/// MCP over HTTP. Feature `server`, because it needs axum.
#[cfg(feature = "server")]
pub mod mcp_http;
/// MCP over stdio. Framing only: the protocol lives in `gabion::mcp`, because
/// `gabion` must stay wasm-safe and this needs a file descriptor.
pub mod mcp_stdio;
/// The stream surface: one WebSocket per host, multiplexed by namespace.
#[cfg(feature = "server")]
pub mod ws;
#[cfg(feature = "server")]
pub use mcp_http::{boxed_commands, host_router, mcp_router};
#[cfg(feature = "server")]
pub use ws::ws_router;

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

/// Real timer over `tokio::time`. A host drives it from a tokio runtime; outside
/// one tokio panics loudly, which beats waiting for a clock that does not run.
#[derive(Clone, Copy, Default)]
pub struct TokioSleep;

impl Sleep for TokioSleep {
    fn sleep(&self, millis: u64) -> Timer {
        Box::pin(tokio::time::sleep(Duration::from_millis(millis)))
    }

    /// On a real clock a deadline and a wait are the same timer.
    fn timer(&self, millis: u64) -> Timer {
        self.sleep(millis)
    }
}

/// Real http client. This is the effect a cross-feature call travels on: the
/// core names a URL, this names the library.
#[derive(Clone, Default)]
pub struct ReqwestHttp {
    client: reqwest::Client,
}

impl ReqwestHttp {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Http for ReqwestHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut call = match request.method {
            HttpMethod::Get => self.client.get(&request.url),
            HttpMethod::Post => self.client.post(&request.url),
            HttpMethod::Put => self.client.put(&request.url),
            HttpMethod::Delete => self.client.delete(&request.url),
        };
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let reply = call
            .body(request.body)
            .send()
            .await
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        let status = reply.status().as_u16();
        // Header values that are not UTF-8 are dropped: v1 carries JSON, and a
        // caller that needs raw bytes gets them from the body.
        let headers = reply
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.to_string(), v.to_string()))
            })
            .collect();
        let body = reply
            .bytes()
            .await
            .map_err(|e| HttpError::Transport(e.to_string()))?
            .to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// The native host's capability bundle.
#[derive(Clone)]
pub struct NativeCaps {
    clock: SystemClock,
    store: InMemoryStore,
    random: SystemRandom,
    sleep: TokioSleep,
    http: ReqwestHttp,
    streams: WsStreams,
}

impl NativeCaps {
    #[must_use]
    pub fn new() -> Self {
        Self {
            clock: SystemClock,
            store: InMemoryStore::default(),
            random: SystemRandom,
            sleep: TokioSleep,
            http: ReqwestHttp::new(),
            streams: WsStreams::new(),
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
    type Sleep = TokioSleep;
    type Http = ReqwestHttp;
    type Streams = WsStreams;
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

/// Persistent bundle for CLI hosts (state survives process exit).
#[derive(Clone)]
pub struct FileCaps {
    clock: SystemClock,
    store: FileStore,
    random: SystemRandom,
    sleep: TokioSleep,
    http: ReqwestHttp,
    streams: WsStreams,
}

impl FileCaps {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Result<Self, gabion::caps::StoreError> {
        Ok(Self {
            clock: SystemClock,
            store: FileStore::new(path)?,
            random: SystemRandom,
            sleep: TokioSleep,
            http: ReqwestHttp::new(),
            streams: WsStreams::new(),
        })
    }
}

impl Caps for FileCaps {
    type Clock = SystemClock;
    type Store = FileStore;
    type Random = SystemRandom;
    type Sleep = TokioSleep;
    type Http = ReqwestHttp;
    type Streams = WsStreams;
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

// (StoreError stays at gabion::caps::StoreError — hosts use that path.)
