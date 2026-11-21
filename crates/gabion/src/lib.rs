//! gabion runtime: capability traits, error model, prelude.
//!
//! This crate must stay wasm-safe and runtime-free: no tokio, no std::fs/net/time.
//! It is imported by every *-api and *-core crate in a gabion app.

pub mod caps;
pub mod channel;
pub mod cli;
pub mod client;
pub mod conformance;
pub mod ctx;
pub mod error;
pub mod frames;
pub mod mcp;
pub mod openapi;
pub mod registry;
pub mod stream;
pub mod testing;
pub mod ws;

pub use ctx::Ctx;
pub use gabion_macros::service;
pub use gabion_macros::service_impl;

/// Re-exported so app crates don't pin serde/serde_json/schemars themselves.
pub use async_trait::async_trait;
pub use schemars;
pub use serde;
pub use serde_json;

pub mod prelude {
    pub use crate::async_trait;
    pub use crate::caps::{
        timeout, Caps, Channel, Clock, Http, HttpError, HttpMethod, HttpRequest, HttpResponse,
        NoStreams, Random, Sleep, Store, StreamError, Streams, TimedOut, Timer,
    };
    pub use crate::ctx::{Cancel, Ctx};
    pub use crate::error::{ErrorKind, FromWire, ServiceError, WireError};
    pub use crate::frames::{Frame, StreamId};
    pub use crate::service;
    pub use crate::service_impl;
    pub use crate::stream::{BoxStream, Each, EndReason, Latest, Stream};
    pub use crate::ws::{Connection, Op, Opened, Policy, WsService};
    pub use schemars::JsonSchema;
    pub use serde::{Deserialize, Serialize};
}
