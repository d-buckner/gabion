//! Browser capability implementations, for a host that compiles to wasm.
//!
//! # Transports only
//!
//! The collab design
//! ([the gabion repository's `docs/local-first-collab.md`, §4.1](https://github.com/d-buckner/gabion/blob/main/docs/local-first-collab.md))
//! scopes this crate to transports: the browser `WebSocket` today, a WebRTC
//! `RTCDataChannel` when collaboration lands. Networking is an effect, so it
//! lives here the way `FileStore` lives in `gabion-native`. The rest of a
//! browser capability bundle — a js clock, localStorage, `fetch` — stays in the
//! host that wants it, because it is that app's storage and not the
//! framework's.
//!
//! # Not an effect home
//!
//! `gabion-native` is a designated effect home: it holds `std::time`, `std::fs`,
//! `std::env` and threads, so it carries the one crate-wide
//! `#![allow(clippy::disallowed_types, clippy::disallowed_methods)]` the effect
//! ban needs. This crate holds none of those. Every effect here is a browser API
//! reached through `web_sys`, so there is nothing to opt out of, and this crate
//! declares no allow.

/// The stream transport: one browser `WebSocket` per channel, written as frames.
pub mod streams;
pub use streams::WebStreams;
