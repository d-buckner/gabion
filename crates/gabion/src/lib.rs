//! gabion runtime: capability traits, error model, prelude.
//!
//! This crate must stay wasm-safe and runtime-free: no tokio, no std::fs/net/time.
//! It is imported by every *-api and *-core crate in a gabion app.

pub mod caps;
pub mod cli;
pub mod client;
pub mod conformance;
pub mod ctx;
pub mod error;
pub mod mcp;
pub mod openapi;
pub mod testing;

pub use ctx::Ctx;
pub use gabion_macros::service;

/// Re-exported so app crates don't pin serde/serde_json/schemars themselves.
pub use async_trait::async_trait;
pub use schemars;
pub use serde;
pub use serde_json;

pub mod prelude {
    pub use crate::async_trait;
    pub use crate::caps::{
        timeout, Caps, Clock, Http, HttpError, HttpMethod, HttpRequest, HttpResponse, Random,
        Sleep, Store, TimedOut, Timer,
    };
    pub use crate::ctx::{Cancel, Ctx};
    pub use crate::error::{ErrorKind, FromWire, ServiceError, WireError};
    pub use crate::service;
    pub use schemars::JsonSchema;
    pub use serde::{Deserialize, Serialize};
}
