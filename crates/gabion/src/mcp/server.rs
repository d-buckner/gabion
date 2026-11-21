//! The JSON-RPC envelope: one request in, at most one response out.
//!
//! Nothing here knows what a tool does. A `Server` owns the mounted services and
//! answers the four methods an MCP client uses; the transport (stdio, HTTP, a
//! test harness) owns bytes. Splitting it this way is what lets the same server be
//! served by `gabion-native`'s stdio pump and by an axum route without the
//! protocol existing twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::envelope::{error_response, initialize, render, result};
use super::{stream, Discard, Emit, Service, Tool};
use crate::ctx::Ctx;
use crate::error::ErrorKind;

/// Every mounted service, answering JSON-RPC requests.
///
/// Cloning a `Server` is cheap and shares the in-flight registry: a transport that
/// serves requests concurrently (the stdio pump, an axum endpoint) clones it per
/// task so a `notifications/cancelled` can reach a call that is still running.
#[derive(Clone, Default)]
pub struct Server {
    services: Vec<Arc<dyn Service>>,
    /// Calls in flight, keyed by request id, so a cancellation notification can
    /// find the one it names. Cancellation is a notification per the MCP spec, so
    /// it carries no reply — the cancelled call answers for itself.
    in_flight: Arc<Mutex<HashMap<String, Ctx>>>,
    /// Cancellations that named a request this server has not started yet.
    ///
    /// Two requests are dispatched concurrently, so "cancel" can genuinely arrive
    /// before "call" — measured: a client that gives up immediately after sending
    /// hits that ordering every time. Dropping the notification would leave the
    /// call running for its own timeout while the agent believes it stopped, so it
    /// is remembered until the call registers. Bounded, because an id that never
    /// arrives must not grow a map forever.
    early_cancels: Arc<Mutex<Vec<String>>>,
    /// Whether this transport can carry a stream. `POST /mcp` cannot: one JSON
    /// answer holds one value, and v1 has no SSE. A server built for it refuses a
    /// listen op by name instead of answering with something strange (§9).
    streams: bool,
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: true,
            ..Default::default()
        }
    }

    /// A server for a transport that answers once per request. Listen ops are
    /// hidden from `tools/list` and refused by name on call.
    #[must_use]
    pub fn without_streams(mut self) -> Self {
        self.streams = false;
        self
    }

    /// Why a listen op is not offered here, in the words an agent reads.
    const NO_STREAM_REASON: &'static str = "this MCP transport answers one request with one JSON \
        value, and a listen op produces many; v1 has no streaming transport here. Use the \
        WebSocket surface (`GET /api/ws`) or the CLI subcommand.";

    /// Mount one service. Mounting is explicit: a service that nothing mounted
    /// is not reachable, which is what makes `gabion::registry` able to report
    /// the ones nobody served.
    #[must_use]
    pub fn with(mut self, service: Arc<dyn Service>) -> Self {
        self.services.push(service);
        self
    }

    /// Every tool from every mounted service, sorted by name so the list is a
    /// stable reviewable artifact rather than mount order.
    #[must_use]
    pub fn tools(&self) -> Vec<Tool> {
        let mut out: Vec<Tool> = self
            .services
            .iter()
            .flat_map(|s| {
                let hidden = s.stream_ops();
                s.tools()
                    .into_iter()
                    .filter(move |t| self.streams || !hidden.iter().any(|o| o == &t.name))
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Answer one request. `None` means "this was a notification": JSON-RPC has
    /// no response for those, and answering one anyway is a protocol error a
    /// client will report as a broken stream.
    #[must_use]
    pub async fn handle(&self, request: &str) -> Option<String> {
        self.handle_with(request, &mut Discard).await
    }

    /// Answer one request, writing any notifications to `emit` before the answer.
    ///
    /// A stream's items go out as `notifications/progress` and the answer reports
    /// how many arrived, so a client that never saw a progress notification knows
    /// the stream produced nothing rather than guessing from an empty answer.
    #[must_use]
    pub async fn handle_with(&self, request: &str, emit: &mut dyn Emit) -> Option<String> {
        let envelope: Value = match serde_json::from_str(request) {
            Ok(v) => v,
            Err(e) => return Some(error_response(Value::Null, -32700, &e.to_string(), None)),
        };
        let id = envelope.get("id").cloned();
        let method = envelope.get("method").and_then(Value::as_str).unwrap_or("");
        let params = envelope.get("params").cloned().unwrap_or(Value::Null);
        // Cancellation arrives as a notification, so it is handled before the
        // "no id means no reply" rule — acting on it *is* the reply.
        if method == "notifications/cancelled" {
            self.cancel(&params);
            return None;
        }
        if id.is_none() || id == Some(Value::Null) {
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        match method {
            "initialize" => Some(result(id, initialize())),
            "tools/list" => Some(result(id, json!({ "tools": self.tools_json() }))),
            "tools/call" => {
                let key = id.to_string();
                let ctx = Ctx::from_meta(params.get("_meta").unwrap_or(&Value::Null));
                self.remember(&key, &ctx);
                let reply = self.call(id, &params, &ctx, emit).await;
                self.forget(&key);
                Some(reply)
            }
            "ping" => Some(result(id, json!({}))),
            other => Some(error_response(
                id,
                -32601,
                &format!("unknown method `{other}`"),
                None,
            )),
        }
    }

    /// The `tools/list` payload, shared with the bearer of an HTTP endpoint that
    /// wants to answer it without a full envelope.
    #[must_use]
    pub fn tools_json(&self) -> Vec<Value> {
        self.tools()
            .into_iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect()
    }

    /// How many early cancellations to hold. Enough for a client that gives up on a
    /// burst of calls; small enough that an id nobody ever sends cannot leak.
    const EARLY_CANCEL_CAP: usize = 32;

    /// Record a call so a cancellation can find it — including one that arrived
    /// before the call did.
    fn remember(&self, key: &str, ctx: &Ctx) {
        if let Ok(mut map) = self.in_flight.lock() {
            map.insert(key.to_string(), ctx.clone());
        }
        let early = self.early_cancels.lock().ok().and_then(|mut prior| {
            prior
                .iter()
                .position(|k| k == key)
                .map(|at| prior.remove(at))
        });
        if early.is_some() {
            ctx.cancel();
        }
    }

    fn forget(&self, key: &str) {
        if let Ok(mut map) = self.in_flight.lock() {
            map.remove(key);
        }
    }

    /// Cancel the call the notification names. A tool that observes
    /// `ctx.cancel_token()` stops; one that does not keeps running, which is the
    /// same contract as every other surface: cancellation is offered, not imposed.
    fn cancel(&self, params: &Value) {
        let Some(id) = params.get("requestId").or_else(|| params.get("request_id")) else {
            return;
        };
        let key = id.to_string();
        let ctx = self
            .in_flight
            .lock()
            .ok()
            .and_then(|map| map.get(&key).cloned());
        match ctx {
            Some(ctx) => ctx.cancel(),
            None => self.remember_early_cancel(key),
        }
    }

    fn remember_early_cancel(&self, key: String) {
        let Ok(mut early) = self.early_cancels.lock() else {
            return;
        };
        if early.len() >= Self::EARLY_CANCEL_CAP {
            early.remove(0);
        }
        early.push(key);
    }

    async fn call(&self, id: Value, params: &Value, ctx: &Ctx, emit: &mut dyn Emit) -> String {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let Some(service) = self
            .services
            .iter()
            .find(|s| s.tools().iter().any(|t| t.name == name))
        else {
            return error_response(
                id,
                ErrorKind::NotFound.mcp_code(),
                &format!("no tool named `{name}`"),
                None,
            );
        };
        if service.stream_ops().iter().any(|o| o == name) {
            return stream::stream_call(stream::Request {
                id,
                service: service.as_ref(),
                name,
                args,
                params,
                ctx,
                emit,
                transport_carries_streams: self.streams,
                reason: Self::NO_STREAM_REASON,
            })
            .await;
        }
        match service.call(ctx, name, args).await {
            Ok(value) => result(
                id,
                json!({
                    "content": [{ "type": "text", "text": render(&value) }],
                    "isError": false,
                }),
            ),
            Err(wire) => error_response(id, wire.kind.mcp_code(), &wire.message, Some(json!(wire))),
        }
    }
}
