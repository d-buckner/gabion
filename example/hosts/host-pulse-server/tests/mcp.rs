//! The MCP surface of a running server: over HTTP, and over stdio.
//!
//! One `gabion::mcp::Server` answers both, from the same impls the routes serve.
//! The two transports are tested on purpose: the protocol being shared is the
//! claim, and a transport that silently differs is how an agent ends up with a
//! tool list a browser cannot reproduce.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};

/// A buffer the pump's tasks can own a clone of.
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

use gabion::caps::{Http, HttpRequest};
use serde_json::{json, Value};

/// Serve the host's app on a port the OS picks.
async fn serve() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = host_pulse_server::app();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

async fn rpc(base: &str, request: Value) -> Value {
    let http = gabion_native::ReqwestHttp::new();
    let reply = http
        .send(
            HttpRequest::post(format!("{base}/mcp"), request.to_string().into_bytes())
                .with_header("content-type", "application/json"),
        )
        .await
        .expect("the mcp endpoint answered");
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    serde_json::from_slice(&reply.body).unwrap()
}

#[tokio::test]
async fn one_endpoint_publishes_both_services_tools() {
    let base = serve().await;
    let reply = rpc(&base, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await;
    let tools = reply["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "check.add",
            "check.history",
            "check.list",
            "check.probe",
            "check.sweep",
            "incident.evaluate",
            "incident.list",
            "incident.uptime",
        ],
        "every op in both traits is a tool"
    );
}

/// A tool call runs real business logic: this one adds a watch through the same
/// impl the HTTP route uses, then reads it back through another tool.
#[tokio::test]
async fn a_tool_call_runs_the_same_impl_the_routes_serve() {
    let base = serve().await;
    let added = rpc(
        &base,
        json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"check.add","arguments":{"name":"api","url":"https://example.com"}}
        }),
    )
    .await;
    assert_eq!(added["result"]["isError"], false, "{added}");
    let text = added["result"]["content"][0]["text"].as_str().unwrap();
    let check: Value = serde_json::from_str(text).unwrap();
    assert_eq!(check["name"], "api");

    let listed = rpc(
        &base,
        json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"check.list","arguments":{}}
        }),
    )
    .await;
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    let reports: Value = serde_json::from_str(text).unwrap();
    assert_eq!(reports.as_array().unwrap().len(), 1);
}

/// A notification is answered with `204`, not with an empty JSON body: a client
/// that receives a reply where it expected silence treats the stream as broken.
#[tokio::test]
async fn a_notification_gets_no_content() {
    let base = serve().await;
    let http = gabion_native::ReqwestHttp::new();
    let reply = http
        .send(
            HttpRequest::post(
                format!("{base}/mcp"),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"})
                    .to_string()
                    .into_bytes(),
            )
            .with_header("content-type", "application/json"),
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 204);
    assert!(reply.body.is_empty());
}

/// The stdio transport, driven with buffers. Same server, same answers: the
/// protocol is not re-implemented per transport.
#[test]
fn the_stdio_transport_answers_the_same_server() {
    let server = host_pulse_server::mcp();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
    );
    // The pump dispatches concurrently, so the writer is shared with the tasks it
    // spawns rather than borrowed.
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let writer = SharedBuf(output.clone());
    gabion_native::mcp_stdio::pump(Cursor::new(input), writer, server).unwrap();

    let text = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "two requests, one notification, two replies"
    );

    let init: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "gabion");
    let list: Value = serde_json::from_str(lines[1]).unwrap();
    let tools: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // Stdio carries a stream, so a listen op is a tool here. The count is the
    // point, not the number: an op the trait declares and `tools/list` hides is
    // an op no agent can find.
    assert!(
        tools.contains(&"check.watch") && tools.contains(&"check.tail"),
        "the listen ops must be offered over stdio, got: {tools:?}"
    );
    assert_eq!(
        tools.len(),
        10,
        "every declared op must be listed: {tools:?}"
    );
}
