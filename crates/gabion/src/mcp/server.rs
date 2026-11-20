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

use super::{Service, Tool, PROTOCOL_VERSION};
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
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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
        let mut out: Vec<Tool> = self.services.iter().flat_map(|s| s.tools()).collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Answer one request. `None` means "this was a notification": JSON-RPC has
    /// no response for those, and answering one anyway is a protocol error a
    /// client will report as a broken stream.
    #[must_use]
    pub async fn handle(&self, request: &str) -> Option<String> {
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
                let reply = self.call(id, &params, &ctx).await;
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

    async fn call(&self, id: Value, params: &Value, ctx: &Ctx) -> String {
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

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "gabion",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// A tool result travels as text in v1. JSON is what every tool here returns,
/// and a client that wants structure parses it back out of the one field the
/// spec guarantees.
fn render(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error. `code` comes from `ErrorKind::mcp_code`, so a client that
/// already understands the HTTP status understands this too.
fn error_response(id: Value, code: i32, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let (Some(data), Some(map)) = (data, error.as_object_mut()) {
        map.insert("data".to_string(), data);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error }).to_string()
}
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // a failing assertion should show the payload
mod cancel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::Server;
    use crate::ctx::Ctx;
    use crate::error::WireError;
    use crate::mcp::{Service, Tool, ToolResult};

    /// A tool that runs until it is cancelled, then reports it.
    struct Slow;

    impl Service for Slow {
        fn ns(&self) -> &str {
            "demo"
        }

        fn tools(&self) -> Vec<Tool> {
            vec![Tool {
                name: "demo.slow".to_string(),
                description: "Runs until cancelled.".to_string(),
                input_schema: json!({ "type": "object" }),
            }]
        }

        fn call<'a>(&'a self, ctx: &'a Ctx, _name: &'a str, _args: Value) -> ToolResult<'a> {
            Box::pin(async move {
                let waited =
                    tokio::time::timeout(Duration::from_secs(5), ctx.cancel_token().cancelled());
                let cancelled = waited.await.is_ok();
                Ok::<Value, WireError>(json!({ "cancelled": cancelled }))
            })
        }
    }

    fn server() -> Server {
        Server::new().with(Arc::new(Slow))
    }

    /// The MCP way to stop a long tool call: a `notifications/cancelled` naming the
    /// request id. The call answers itself with what it observed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancellation_notification_stops_the_call_it_names() {
        let server = server();
        let call = Clone::clone(&server);
        let running = tokio::spawn(async move {
            call.handle(
                &json!({
                    "jsonrpc":"2.0","id":7,"method":"tools/call",
                    "params":{"name":"demo.slow","arguments":{}}
                })
                .to_string(),
            )
            .await
        });

        // Give the call a moment to register as in flight.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let reply = server
            .handle(
                &json!({
                    "jsonrpc":"2.0","method":"notifications/cancelled",
                    "params":{"requestId":7,"reason":"the user gave up"}
                })
                .to_string(),
            )
            .await;
        assert!(reply.is_none(), "a notification must not be answered");

        let answered = running
            .await
            .expect("the call task finished")
            .expect("a request with an id is answered");
        let value: Value = serde_json::from_str(&answered).unwrap();
        let text = value["result"]["content"][0]["text"].as_str().unwrap();
        let observed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            observed["cancelled"], true,
            "the tool must see the cancellation: {value}"
        );
    }

    /// The other ordering: the client gives up fast enough that the cancellation is
    /// dispatched before the call it names. Dropping it would leave the call running
    /// while the agent believes it stopped, so the server holds it until the call
    /// registers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancellation_that_arrives_first_is_not_lost() {
        let server = server();
        let notification = json!({
            "jsonrpc":"2.0","method":"notifications/cancelled",
            "params":{"requestId":11}
        })
        .to_string();
        assert!(server.handle(&notification).await.is_none());

        let answered = server
            .handle(
                &json!({
                    "jsonrpc":"2.0","id":11,"method":"tools/call",
                    "params":{"name":"demo.slow","arguments":{}}
                })
                .to_string(),
            )
            .await
            .expect("a request with an id is answered");
        let value: Value = serde_json::from_str(&answered).unwrap();
        let observed: Value =
            serde_json::from_str(value["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            observed["cancelled"], true,
            "a cancellation that beat the call must still stop it: {value}"
        );
    }

    /// A cancellation for an id nobody is running is ignored, not an error: an agent
    /// that gave up on a call that already finished must not get a failure for it.
    #[tokio::test]
    async fn cancelling_an_unknown_request_is_harmless() {
        let server = server();
        let reply = server
            .handle(
                &json!({
                    "jsonrpc":"2.0","method":"notifications/cancelled",
                    "params":{"requestId":999}
                })
                .to_string(),
            )
            .await;
        assert!(reply.is_none());
        // The server still works afterwards.
        let listed = server
            .handle(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string())
            .await
            .unwrap();
        assert!(listed.contains("demo.slow"));
    }
}
