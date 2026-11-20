//! The JSON-RPC envelope: one request in, at most one response out.
//!
//! Nothing here knows what a tool does. A `Server` owns the mounted services and
//! answers the four methods an MCP client uses; the transport (stdio, HTTP, a
//! test harness) owns bytes. Splitting it this way is what lets the same server be
//! served by `gabion-native`'s stdio pump and by an axum route without the
//! protocol existing twice.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{Service, Tool, PROTOCOL_VERSION};
use crate::ctx::Ctx;
use crate::error::ErrorKind;

/// Every mounted service, answering JSON-RPC requests.
#[derive(Default)]
pub struct Server {
    services: Vec<Arc<dyn Service>>,
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
        if id.is_none() || id == Some(Value::Null) {
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        match method {
            "initialize" => Some(result(id, initialize())),
            "tools/list" => Some(result(id, json!({ "tools": self.tools_json() }))),
            "tools/call" => Some(self.call(id, &params).await),
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

    async fn call(&self, id: Value, params: &Value) -> String {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let ctx = Ctx::from_meta(params.get("_meta").unwrap_or(&Value::Null));
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
        match service.call(&ctx, name, args).await {
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
