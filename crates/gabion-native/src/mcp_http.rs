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
///
/// One request, one JSON answer, so a listen op is refused here by name and is not
/// listed as a tool at all. That is the written exception to "every method reaches
/// every surface" (docs/streaming.md §9): v1 has no SSE, and a tool that returned
/// the first item and dropped the rest would be worse than no tool.
pub fn mcp_router(server: Server) -> axum::Router {
    axum::Router::new()
        .route("/mcp", post(handle))
        .with_state(Arc::new(server.without_streams()))
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

/// The whole app a server host serves: every mounted router, plus the MCP
/// endpoint when the host's api crates were built with `mcp`.
///
/// The downcasts are here rather than in the host because the host should not
/// have to know that a generated router is an `axum::Router` — it mounted an impl
/// and asked what it serves.
pub fn host_router(host: &mut gabion::registry::Host) -> axum::Router {
    let mcp = host.mcp();
    let streams = host.ws();
    let mut app = axum::Router::new();
    for boxed in host.routers() {
        if let Ok(router) = boxed.downcast::<axum::Router>() {
            app = app.merge(*router);
        }
    }
    let app = match mcp {
        Some(server) => app.merge(mcp_router(server)),
        None => app,
    };
    // One endpoint for every streaming feature, mounted the way `/mcp` is: a host
    // that mounted a service that streams gets the route, and one that did not
    // gets no route rather than one that answers nothing.
    app.merge(crate::ws::ws_router(streams))
}

/// Every mounted subcommand, for a host that builds one clap tree over several
/// services. Boxed on the way in for the same reason routers are: the registry
/// cannot name `clap`, and this module could — but a host that wants a CLI is the
/// one that has one, so the downcast stays there.
#[must_use]
pub fn boxed_commands(host: &mut gabion::registry::Host) -> Vec<Box<dyn std::any::Any + Send>> {
    host.commands()
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
