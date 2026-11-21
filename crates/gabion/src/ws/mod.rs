//! The WebSocket surface: what a service offers a connection, and the client-side
//! multiplexer that reads one (`docs/streaming.md` §5, §7).
//!
//! Runtime-free, like the rest of this crate. The server half is a trait a
//! generated adapter implements and a host pumps; the client half is a
//! [`Connection`] that any caller can drive from its own task, because gabion
//! cannot spawn and a wasm host has no thread to spawn on.

use crate::ctx::Ctx;
use crate::error::{ErrorKind, WireError};
use crate::stream::{BoxStream, Stream};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

pub mod client;

pub use client::{Connection, Receiver};

/// The flow policy a stream op declared in its return type.
///
/// Read off `Each<T>` or `Latest<T>` by the generator, so this is a fact about a
/// signature and not a knob a host sets at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Every item reaches the subscriber, or the subscriber waits (§7.3).
    Each,
    /// The newest item is what matters, so an untaken one is replaced (§7.2).
    Latest,
}

impl Policy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Each => "each",
            Policy::Latest => "latest",
        }
    }
}

/// One operation a namespace serves, and whether it streams.
#[derive(Debug, Clone)]
pub struct Op {
    /// Dotted, like every other gabion op name: `pulse.watch`.
    pub name: String,
    /// `Some` for a listen op, `None` for an ordinary one.
    pub policy: Option<Policy>,
}

/// What a listen op handed back: the items, and the promise about how they may be
/// read.
pub struct Opened {
    pub policy: Policy,
    pub items: BoxStream<'static, Result<Value, WireError>>,
}

/// One `call` frame's outcome, boxed the way `mcp::ToolResult` boxes a tool's.
pub type CallResult<'a> = Pin<Box<dyn Future<Output = Result<Value, WireError>> + Send + 'a>>;

/// One `open` frame's outcome: the stream, once the op has accepted the call.
pub type OpenResult<'a> = Pin<Box<dyn Future<Output = Result<Opened, WireError>> + Send + 'a>>;

/// One service as the WebSocket surface sees it.
///
/// The adapter is generated next to the MCP adapter and dispatches by op name.
/// A host pumps it; this trait never touches a socket.
pub trait WsService: Send + Sync + 'static {
    /// The namespace every op name starts with.
    fn ns(&self) -> &str;

    /// Every op this namespace serves, in trait order.
    fn ops(&self) -> Vec<Op>;

    /// Run one ordinary op.
    fn call<'a>(&'a self, ctx: &'a Ctx, op: &'a str, input: Value) -> CallResult<'a>;

    /// Start one stream op. An op that does not stream is `NotFound`, the same
    /// answer a route gives for a path it does not serve.
    fn open<'a>(&'a self, ctx: &'a Ctx, op: &'a str, input: Value) -> OpenResult<'a>;

    /// Whether `op` names an operation of this namespace.
    fn serves(&self, op: &str) -> bool {
        self.ops().iter().any(|o| o.name == op)
    }
}

/// Turn a stream of typed items into the stream of JSON values a surface writes.
///
/// Here rather than in every generated adapter, so the one place an item can fail
/// to serialize is one place. An item that will not serialize ends the stream with
/// `Internal` rather than being skipped: a silently missing item is a stream that
/// lies about what it delivered.
pub fn encode_items<T: serde::Serialize + 'static>(
    items: BoxStream<'static, T>,
) -> BoxStream<'static, Result<Value, WireError>> {
    Box::pin(Encoded { items })
}

struct Encoded<T> {
    items: BoxStream<'static, T>,
}

impl<T: serde::Serialize> Stream for Encoded<T> {
    type Item = Result<Value, WireError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.items.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(item)) => Poll::Ready(Some(serde_json::to_value(item).map_err(|e| {
                WireError::new(
                    ErrorKind::Internal,
                    format!("item would not serialize: {e}"),
                )
            }))),
        }
    }
}

/// Turn a decoded stream of `Result<T, E>` into the plain `T` stream a marker holds.
///
/// A mid-stream failure ends the stream. The marker's item type is the domain type,
/// so there is no second channel for a failure that arrives after the first item,
/// and inventing one would mean a marker that is not `Stream<Item = T>`. What the
/// caller can still tell apart is the case that matters: a refusal before any item
/// comes back as `Err` from the call itself, and a stream that stops early is
/// reported by the codec as a transport failure rather than as a clean end.
pub fn decode_items<T, E>(items: BoxStream<'static, Result<T, E>>) -> BoxStream<'static, T>
where
    T: 'static,
    E: 'static,
{
    Box::pin(Decoded { items })
}

struct Decoded<S> {
    items: S,
}

impl<T, E> Stream for Decoded<BoxStream<'static, Result<T, E>>> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Option<T>> {
        match self.get_mut().items.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(item))) => Poll::Ready(Some(item)),
            // Ended, not skipped quietly: the codec already named the failure, and
            // a stream that stops without an `end` is not a completed stream.
            Poll::Ready(Some(Err(_))) => Poll::Ready(None),
        }
    }
}
