//! A listen op over the JSON-RPC transport: one `notifications/progress` per
//! item, then the answer (docs/streaming.md §9).
//!
//! Split from the envelope because the envelope is one request in and one answer
//! out, and this is the one place that breaks the shape. Keeping it in one file
//! would make every reader of the envelope hold a stream loop they do not need.

use serde_json::{json, Value};

use super::envelope::{error_response, result};
use super::{Emit, Service};
use crate::ctx::Ctx;
use crate::error::ErrorKind;
use crate::stream::next;

/// One listen op, as the transport received it.
pub(crate) struct Request<'a> {
    pub(crate) id: Value,
    pub(crate) service: &'a dyn Service,
    pub(crate) name: &'a str,
    pub(crate) args: Value,
    pub(crate) params: &'a Value,
    pub(crate) ctx: &'a Ctx,
    pub(crate) emit: &'a mut dyn Emit,
    /// Whether this transport can carry a stream at all. `POST /mcp` cannot.
    pub(crate) transport_carries_streams: bool,
    /// What to tell a caller who asked anyway, in the words they read.
    pub(crate) reason: &'a str,
}

/// One listen op over stdio: a `notifications/progress` per item, then the
/// answer. Refused when the transport cannot carry it, or when the client gave
/// no `progressToken` to put the items under.
pub(crate) async fn stream_call(request: Request<'_>) -> String {
    let Request {
        id,
        service,
        name,
        args,
        params,
        ctx,
        emit,
        transport_carries_streams,
        reason,
    } = request;
    if !transport_carries_streams {
        return error_response(id, ErrorKind::Forbidden.mcp_code(), reason, None);
    }
    let Some(token) = params_token(params) else {
        return error_response(
            id,
            ErrorKind::InvalidInput.mcp_code(),
            "a listen op needs `_meta.progressToken` on the request: the items are sent as \
                 progress notifications, and without a token there is nothing to send them under",
            None,
        );
    };
    let started = match service.stream(ctx, name, args).await {
        Ok(items) => items,
        Err(wire) => {
            return error_response(id, wire.kind.mcp_code(), &wire.message, Some(json!(wire)))
        }
    };
    let mut items = started;
    let mut count = 0u64;
    let mut end = "done";
    while let Some(item) = next(&mut items).await {
        match item {
            Ok(value) => {
                count += 1;
                emit.emit(progress(&token, count, value)).await;
            }
            Err(wire) => {
                return error_response(id, wire.kind.mcp_code(), &wire.message, Some(json!(wire)));
            }
        }
        if ctx.cancel_token().is_cancelled() {
            end = "cancelled";
            break;
        }
    }
    result(
        id,
        json!({
            "content": [{ "type": "text", "text": format!("streamed {count} items") }],
            "structuredContent": { "items": count, "end": end },
            "isError": false,
        }),
    )
}

/// The `progressToken` a client offered, read from `params._meta` the way
/// `_meta` is read everywhere else in this server.
fn params_token(params: &Value) -> Option<Value> {
    params
        .get("_meta")
        .and_then(|m| m.get("progressToken"))
        .cloned()
}

/// One item, as the notification that carries it.
///
/// `message` is a string in the MCP schema, so the item travels as its own
/// compact JSON text. A client that wants structure parses it back; gabion does
/// not invent a second notification the spec does not define.
fn progress(token: &Value, at: u64, item: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": { "progressToken": token, "progress": at, "message": item.to_string() },
    })
    .to_string()
}
