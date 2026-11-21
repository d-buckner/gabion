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

use serde::Serialize;
use serde_json::{json, Value};

use crate::ctx::Ctx;
use crate::error::{ErrorKind, ServiceError, WireError};

/// What a tool call resolves to: the op's JSON value, or the flat wire error
/// every other surface sends. Boxed because the trait is object-safe and this
/// crate takes no `async_trait` dependency.
pub type ToolResult<'a> = Pin<Box<dyn Future<Output = Result<Value, WireError>> + Send + 'a>>;

/// The protocol revision this server speaks. MCP versions are dates; a client
/// that asks for a different one is told what it got rather than refused,
/// because every field this server reads has been stable across them.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

mod server;
pub use server::Server;

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

/// Input schemas for one service's tools, built once per `tools/list` call.
///
/// A separate type so the generated code does not have to know how schemars is
/// configured, and so the generator's settings live in one place.
///
/// The one setting that matters: subschemas are inlined. An `inputSchema` is a
/// self-contained document an agent hands to a model — a `$ref` into a
/// `components/schemas` map that never travels with it is a schema nothing can
/// resolve. (OpenAPI does the opposite, and is right to: there the definitions
/// travel in the same document.)
pub struct SchemaBuilder {
    generator: schemars::generate::SchemaGenerator,
}

impl Default for SchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            generator: schemars::generate::SchemaSettings::draft2020_12()
                .with(|s| {
                    s.meta_schema = None;
                    s.inline_subschemas = true;
                })
                .into_generator(),
        }
    }

    /// The schema for one input type. A schema that will not serialize says so
    /// in the document rather than becoming `null`, which would tell a client
    /// "any arguments are fine".
    pub fn value<T: schemars::JsonSchema>(&mut self) -> Value {
        let schema = self.generator.subschema_for::<T>();
        match serde_json::to_value(schema) {
            Ok(value) => value,
            Err(e) => json!({
                "type": "object",
                "x-gabion-error": format!("schema did not serialize: {e}"),
            }),
        }
    }
}

/// The schema for a tool that takes no arguments. `{}` would mean "any
/// arguments"; this says what the tool expects, which is nothing.
#[must_use]
pub fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// A successful tool result as JSON. A value that will not serialize is an
/// `Internal` failure, never an empty success: the two are indistinguishable to
/// a client otherwise.
pub fn value_of<T: Serialize>(value: &T) -> Result<Value, WireError> {
    serde_json::to_value(value).map_err(|e| {
        WireError::new(
            ErrorKind::Internal,
            format!("result did not serialize: {e}"),
        )
    })
}

/// A failed call as the flat wire error every other surface sends, so the kind
/// is the same one that becomes a 404 and exit 3.
#[must_use]
pub fn wire_of<E: Serialize>(error: &ServiceError<E>) -> WireError {
    WireError::from_service(error)
        .unwrap_or_else(|_| WireError::new(ErrorKind::Internal, "error detail failed to serialize"))
}

/// Parse a tool's `arguments` into the type the method declares. A caller that
/// sends the wrong shape gets `InvalidInput` naming the tool, not a call with
/// default values.
pub fn arguments<T: serde::de::DeserializeOwned>(tool: &str, args: Value) -> Result<T, WireError> {
    serde_json::from_value(args).map_err(|e| {
        WireError::new(
            ErrorKind::InvalidInput,
            format!("bad arguments for {tool}: {e}"),
        )
    })
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

// tokio does not build for wasm32, and these tests need a real executor.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)] // a failing assertion should show the payload
mod tests {
    use std::sync::Arc;

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
