//! MCP over HTTP: one endpoint, one JSON-RPC message per request.
//!
//! This is the streamable-HTTP shape minus the optional SSE stream (a v1 tool
//! call answers once and is done). It exists so an agent that cannot spawn a
//! process can still reach the same tools the stdio transport serves, over the
//! same `gabion::mcp::Server` — the protocol is not duplicated per transport.
//!
//! Feature-gated on `server`: a host that serves an HTTP API has axum, and a host
//! that does not should not pay for it.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;

use gabion::mcp::Server;

/// `POST /mcp` over every mounted service.
///
/// A notification answers `204 No Content` — JSON-RPC has no reply for one, and a
/// client that receives an empty JSON body where it expected silence is entitled
/// to treat it as a protocol error.
pub fn mcp_router(server: Arc<Server>) -> axum::Router {
    axum::Router::new()
        .route("/mcp", post(handle))
        .with_state(server)
}

async fn handle(State(server): State<Arc<Server>>, body: String) -> axum::response::Response {
    match server.handle(&body).await {
        Some(reply) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            reply,
        )
            .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// The same server, answering a request a host received some other way (a test,
/// a websocket, an embedded webview). No axum types, so a host that serves JSON-RPC
/// elsewhere does not have to route it through this module.
pub async fn answer(server: &Server, request: &str) -> Json<Option<serde_json::Value>> {
    let reply = server
        .handle(request)
        .await
        .and_then(|text| serde_json::from_str(&text).ok());
    Json(reply)
}
