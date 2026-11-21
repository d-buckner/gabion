//! The frame envelope every stream rides on: one JSON object per WebSocket text
//! frame, in both directions (`docs/streaming.md` §5).
//!
//! This is gabion's own envelope, not a protocol borrowed from elsewhere, and it
//! is small on purpose: seven shapes, one of which names an operation the way a
//! route, an MCP tool and a CLI subcommand already name it.

use crate::error::WireError;
use crate::stream::EndReason;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which stream on the connection a frame belongs to.
///
/// Opaque to the transport and chosen by whoever opened the stream. One
/// connection carries every feature's streams, so the id plus the namespace is
/// what the open-stream record is keyed by (§5).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct StreamId(pub u64);

impl std::fmt::Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One frame, either direction.
///
/// `op` is the tool name gabion already mints (`{ns}.{method}`), so a frame names
/// an operation the way every other gabion surface names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// Client to server: start the stream `op` names, and stream under `id`.
    Open {
        id: StreamId,
        op: String,
        #[serde(default)]
        input: Value,
        /// The caller, as `Ctx::to_meta`. §7.6: a frame is a call like any other,
        /// so it carries who is calling. Absent means an anonymous caller, which
        /// is what a timer or a test is.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// Client to server: an ordinary op of a feature that streams, sent on the
    /// connection instead of as a separate POST (§5).
    Call {
        id: StreamId,
        op: String,
        #[serde(default)]
        input: Value,
        /// The caller, as `Ctx::to_meta`.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        meta: Value,
    },
    /// Client to server: stop a stream this client opened.
    Close { id: StreamId },
    /// Server to client: one item, tagged with the namespace it came from so a
    /// client cannot read another feature's events by accident (§5).
    Item {
        id: StreamId,
        ns: String,
        data: Value,
    },
    /// Server to client: the result of a `call`.
    Reply { id: StreamId, ok: Value },
    /// Server to client: the stream ended, and why. This is the only clean end.
    End { id: StreamId, reason: EndReason },
    /// Server to client: the stream or the call failed.
    Error { id: StreamId, error: WireError },
}

impl Frame {
    /// The frame a client writes to start a stream.
    #[must_use]
    pub fn open(id: StreamId, op: impl Into<String>, input: Value, ctx: &crate::ctx::Ctx) -> Self {
        Frame::Open {
            id,
            op: op.into(),
            input,
            meta: ctx.to_meta(),
        }
    }

    /// The frame a client writes to run one ordinary op of a feature that streams.
    #[must_use]
    pub fn call(id: StreamId, op: impl Into<String>, input: Value, ctx: &crate::ctx::Ctx) -> Self {
        Frame::Call {
            id,
            op: op.into(),
            input,
            meta: ctx.to_meta(),
        }
    }

    /// The caller a frame names, or an anonymous one when it names nobody.
    #[must_use]
    pub fn ctx(&self) -> crate::ctx::Ctx {
        match self {
            Frame::Open { meta, .. } | Frame::Call { meta, .. } => crate::ctx::Ctx::from_meta(meta),
            _ => crate::ctx::Ctx::anonymous(),
        }
    }

    /// The stream this frame belongs to.
    #[must_use]
    pub fn id(&self) -> StreamId {
        match self {
            Frame::Open { id, .. }
            | Frame::Call { id, .. }
            | Frame::Close { id }
            | Frame::Item { id, .. }
            | Frame::Reply { id, .. }
            | Frame::End { id, .. }
            | Frame::Error { id, .. } => *id,
        }
    }

    /// The namespace half of an op name: `terminal.watch` is `terminal`.
    ///
    /// This is the one comparison the namespace wall is made of (§5), so it is
    /// written once here rather than spelled out at each end.
    #[must_use]
    pub fn op_namespace(op: &str) -> Option<&str> {
        op.split('.')
            .next()
            .filter(|ns| !ns.is_empty() && *ns != op)
    }

    /// The namespace this frame names, if it names one.
    #[must_use]
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Frame::Open { op, .. } | Frame::Call { op, .. } => Self::op_namespace(op),
            Frame::Item { ns, .. } => Some(ns.as_str()),
            Frame::Close { .. } | Frame::Reply { .. } | Frame::End { .. } | Frame::Error { .. } => {
                None
            }
        }
    }

    /// Control frames are handled immediately and out of order; data frames are
    /// dispatched in arrival order per namespace (§5).
    #[must_use]
    pub fn is_control(&self) -> bool {
        matches!(self, Frame::Close { .. })
    }
}

/// Why a channel failed at the transport level.
///
/// A failed *op* is not this: that arrives as a [`Frame::Error`] carrying a
/// [`WireError`], so the app's own error kind survives. This is only for the
/// channel itself.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// The channel never opened, or died mid-stream without an `end` or `error`
    /// frame. An end that is not stated is not a clean end (§7.4).
    #[error("stream transport: {0}")]
    Transport(String),
    /// A frame arrived that gabion cannot act on: it does not parse, it names a
    /// namespace that is not the one this stream opened under, or it is a shape
    /// this end never receives.
    #[error("stream protocol: {0}")]
    Protocol(String),
}

impl StreamError {
    /// Project a channel failure onto the error model, so a client that only has
    /// a [`FromWire`](crate::error::FromWire) can still answer with something
    /// typed. A channel that died is `Internal`: it is the framework's failure,
    /// not the app's, and no app error enum should have to invent a variant for
    /// it.
    #[must_use]
    pub fn to_wire(&self) -> WireError {
        WireError::new(crate::error::ErrorKind::Internal, self.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the frame that broke
mod tests {
    use super::*;
    use crate::error::{ErrorKind, WireError};

    #[test]
    fn a_frame_is_one_json_object_tagged_by_shape() {
        let frame = Frame::Item {
            id: StreamId(7),
            ns: "todo".into(),
            data: serde_json::json!({"title": "milk"}),
        };
        let json = serde_json::to_string(&frame).expect("serialize");
        assert!(
            json.contains("\"type\":\"item\""),
            "the shape is the discriminator: {json}"
        );
        let back: Frame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, frame);
    }

    #[test]
    fn every_shape_round_trips() {
        let ctx = crate::ctx::Ctx::anonymous().with_principal("agent-7");
        let frames = vec![
            Frame::open(StreamId(1), "todo.watch", serde_json::json!({}), &ctx),
            Frame::call(
                StreamId(2),
                "todo.create",
                serde_json::json!({"title": "milk"}),
                &crate::ctx::Ctx::anonymous(),
            ),
            Frame::Close { id: StreamId(3) },
            Frame::Item {
                id: StreamId(4),
                ns: "todo".into(),
                data: serde_json::json!(1),
            },
            Frame::Reply {
                id: StreamId(5),
                ok: serde_json::json!(true),
            },
            Frame::End {
                id: StreamId(6),
                reason: EndReason::Cancelled,
            },
            Frame::Error {
                id: StreamId(7),
                error: WireError::new(ErrorKind::Forbidden, "not your namespace"),
            },
        ];
        for frame in frames {
            let json = serde_json::to_string(&frame).expect("serialize");
            let back: Frame = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, frame, "{json}");
            assert_eq!(back.id(), frame.id());
        }
    }

    /// §7.6: a frame is a call like any other, so it carries who is calling. A
    /// surface that filled in an anonymous `Ctx` for every frame would make every
    /// stream caller the same anonymous principal.
    #[test]
    fn a_frame_carries_who_is_calling() {
        let ctx = crate::ctx::Ctx::anonymous()
            .with_principal("agent-7")
            .with_tenant("acme");
        let frame = Frame::open(StreamId(1), "todo.watch", Value::Null, &ctx);
        let back: Frame =
            serde_json::from_str(&serde_json::to_string(&frame).expect("json")).expect("frame");
        let read = back.ctx();
        assert_eq!(read.principal(), Some("agent-7"));
        assert_eq!(read.tenant(), Some("acme"));

        let anonymous = Frame::Close { id: StreamId(2) };
        assert_eq!(
            anonymous.ctx().principal(),
            None,
            "a frame that names no caller is an anonymous one, not a guess"
        );
    }

    /// The namespace wall is one comparison, and it is this one (§5).
    #[test]
    fn an_op_name_splits_into_namespace_and_method() {
        assert_eq!(Frame::op_namespace("todo.watch"), Some("todo"));
        assert_eq!(Frame::op_namespace("a.b.c"), Some("a"));
        assert_eq!(Frame::op_namespace("watch"), None);
        assert_eq!(Frame::op_namespace(""), None);
    }

    #[test]
    fn only_close_is_a_control_frame() {
        assert!(Frame::Close { id: StreamId(1) }.is_control());
        assert!(!Frame::is_control(&Frame::End {
            id: StreamId(1),
            reason: EndReason::Done,
        }));
    }

    #[test]
    fn a_channel_failure_names_itself_as_internal() {
        let wire = StreamError::Transport("socket closed".into()).to_wire();
        assert_eq!(wire.kind, ErrorKind::Internal);
        assert!(wire.message.contains("socket closed"), "{}", wire.message);
    }
}
