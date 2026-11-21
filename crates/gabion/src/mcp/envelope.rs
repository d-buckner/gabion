//! The JSON-RPC shapes every answer is built from.
//!
//! Small, and separate for that reason: the envelope is the part a reader skips
//! once they know it, and it is the part a reviewer of the stream loop never needs.

use serde_json::{json, Value};

use super::PROTOCOL_VERSION;

pub(crate) fn initialize() -> Value {
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
pub(crate) fn render(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error. `code` comes from `ErrorKind::mcp_code`, so a client that
/// already understands the HTTP status understands this too.
pub(crate) fn error_response(id: Value, code: i32, message: &str, data: Option<Value>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let (Some(data), Some(map)) = (data, error.as_object_mut()) {
        map.insert("data".to_string(), data);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error }).to_string()
}
// tokio does not build for wasm32, and these tests need a real executor.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)] // a failing assertion should show the payload
mod cancel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::super::Server;
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
