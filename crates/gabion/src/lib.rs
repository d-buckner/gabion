//! gabion runtime: capability traits, error model, prelude.
//!
//! This crate must stay wasm-safe and runtime-free: no tokio, no std::fs/net/time.
//! It is imported by every *-api and *-core crate in a gabion app.

pub mod caps;
pub mod cli;
pub mod error;
pub mod testing;

pub use gabion_macros::service;

/// Re-exported so app crates don't pin serde/serde_json themselves.
pub use async_trait::async_trait;
pub use serde;
pub use serde_json;

pub mod prelude {
    pub use crate::async_trait;
    pub use crate::caps::{Caps, Clock, Store};
    pub use crate::error::{ErrorKind, ServiceError};
    pub use crate::service;
    pub use serde::{Deserialize, Serialize};
}
