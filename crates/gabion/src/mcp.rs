//! MCP surface support: the JSON-RPC 2.0 envelope, the tool list a service
//! publishes, and the dispatch that turns a tool call into a service call.
//!
//! One runtime, no SDK. The MCP wire format that matters here is small —
//! `initialize`, `tools/list`, `tools/call` — and every one of those paths is
//! something this framework has to test anyway, because the *error mapping* is
//! the interesting part: `ErrorKind::NotFound` has to become the same JSON-RPC
//! code the HTTP surface turns into 404 and the CLI into exit 3. A dependency
//! would own that mapping, and the framework's whole claim is that one enum maps
//! to every surface.
//!
//! Transport lives elsewhere on purpose. This module never touches stdio, a
//! socket, or a reactor: `gabion-native` serves it over stdio and a host serves
//! it over HTTP, so a browser build can carry it too.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};

/// What a tool call resolves to: the op's JSON value, or the flat wire error
/// every other surface sends. Boxed because the trait is object-safe and this
/// crate takes no `async_trait` dependency.
pub type ToolResult<'a> = Pin<Box<dyn Future<Output = Result<Value, WireError>> + Send + 'a>>;

use crate::ctx::Ctx;
use crate::error::{ErrorKind, WireError};

/// The protocol revision this server speaks. MCP versions are dates; a client
/// that asks for a different one is told what it got rather than refused,
/// because every field this server reads has been stable across them.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// One tool, as `tools/list` publishes it. Built from the same method the HTTP
/// route serves and the CLI dispatches, so the description a client reads is the
/// doc comment a reviewer reads.
#[derive(Debug, Clone)]
pub struct Tool {
    /// Dotted, like the operation id: `todo.create`.
    pub name: String,
    pub description: String,
    /// JSON Schema for `arguments`, from the type the route deserializes.
    pub input_schema: Value,
}

/// One service as MCP sees it.
///
/// `call` is a boxed future rather than an `async fn` because this trait has to
/// be dyn-compatible and object-safe, and it is the one place the runtime
/// accepts a hand-written future: the alternative is an `async_trait` dependency
/// in a crate that must stay runtime-free.
pub trait Service: Send + Sync {
    /// The namespace the tools are namespaced under (`todo`).
    fn ns(&self) -> &str;

    /// Every tool this service publishes, in trait order.
    fn tools(&self) -> Vec<Tool>;

    /// Run one tool. `name` is the full dotted name, so a call for another
    /// service's tool is an error rather than a silently ignored argument.
    fn call<'a>(&'a self, ctx: &'a Ctx, name: &'a str, args: Value) -> ToolResult<'a>;
}

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

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a failing assertion should show the payload
mod tests {
    use super::*;

    struct Stub;

    impl Service for Stub {
        fn ns(&self) -> &str {
            "demo"
        }

        fn tools(&self) -> Vec<Tool> {
            vec![
                Tool {
                    name: "demo.echo".to_string(),
                    description: "Echo one word.".to_string(),
                    input_schema: json!({ "type": "object" }),
                },
                Tool {
                    name: "demo.fail".to_string(),
                    description: "Always fails.".to_string(),
                    input_schema: json!({ "type": "object" }),
                },
            ]
        }

        fn call<'a>(&'a self, ctx: &'a Ctx, name: &'a str, args: Value) -> ToolResult<'a> {
            let fail = name == "demo.fail";
            let caller = ctx.principal().unwrap_or("anonymous").to_string();
            Box::pin(async move {
                if fail {
                    return Err(WireError::new(ErrorKind::NotFound, "nothing there"));
                }
                Ok(json!({ "heard": args, "from": caller }))
            })
        }
    }

    fn server() -> Server {
        Server::new().with(Arc::new(Stub))
    }

    async fn ask(request: Value) -> Value {
        let text = server().handle(&request.to_string()).await.unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn initialize_names_the_protocol_and_the_server() {
        let reply = ask(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})).await;
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(reply["result"]["serverInfo"]["name"], "gabion");
        assert_eq!(reply["id"], 1);
    }

    #[tokio::test]
    async fn tools_list_publishes_one_entry_per_op() {
        let reply = ask(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "demo.echo");
        assert_eq!(tools[0]["description"], "Echo one word.");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[tokio::test]
    async fn a_tool_call_carries_the_caller_from_meta() {
        let reply = ask(json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"demo.echo","arguments":{"word":"hi"},"_meta":{"principal":"agent-7"}}
        }))
        .await;
        assert_eq!(reply["result"]["isError"], false);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(value["heard"]["word"], "hi");
        assert_eq!(value["from"], "agent-7");
    }

    /// The point of the shared error model: the same kind that is 404 over HTTP
    /// is -32001 here, and the flat `WireError` rides along as `data`.
    #[tokio::test]
    async fn a_failed_tool_maps_the_kind_to_an_mcp_code() {
        let reply = ask(json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"demo.fail","arguments":{}}
        }))
        .await;
        assert_eq!(reply["error"]["code"], ErrorKind::NotFound.mcp_code());
        assert_eq!(reply["error"]["message"], "nothing there");
        assert_eq!(reply["error"]["data"]["kind"], "not_found");
    }

    #[tokio::test]
    async fn an_unknown_tool_is_a_not_found_rather_than_an_empty_success() {
        let reply = ask(json!({
            "jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"demo.nope","arguments":{}}
        }))
        .await;
        assert_eq!(reply["error"]["code"], ErrorKind::NotFound.mcp_code());
    }

    #[tokio::test]
    async fn an_unknown_method_is_a_protocol_error() {
        let reply = ask(json!({"jsonrpc":"2.0","id":6,"method":"resources/list"})).await;
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_notification_gets_no_response() {
        let reply = server()
            .handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string())
            .await;
        assert!(reply.is_none(), "a notification must not be answered");
    }

    #[tokio::test]
    async fn a_malformed_request_is_a_parse_error() {
        let reply = server().handle("{not json").await.unwrap();
        let value: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(value["error"]["code"], -32700);
    }
}
