//! The `Http` capability is what a cross-feature call travels on. This drives it
//! through generic `Caps` code against a real server built from the generated
//! router, so the claim is checked end to end rather than against a stub.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{Caps, Http, HttpError, HttpRequest};
use todo_api::Todo;

/// The shape a generated cross-feature client takes: a path, the caller's own
/// `Http`, and no http library in the core.
async fn call<C: Caps>(caps: &C, base: &str, path: &str) -> Result<Todo, HttpError> {
    let body = serde_json::to_vec(&serde_json::json!({ "title": "buy sandspice" })).unwrap();
    let reply = caps
        .http()
        .send(
            HttpRequest::post(format!("{base}{path}"), body)
                .with_header("content-type", "application/json"),
        )
        .await?;
    if !reply.is_success() {
        return Err(HttpError::Transport(format!(
            "status {}: {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        )));
    }
    serde_json::from_slice(&reply.body).map_err(|e| HttpError::Transport(e.to_string()))
}

/// Start the generated router on a port the OS picks.
async fn serve() -> (String, gabion_native::NativeCaps) {
    let caps = gabion_native::NativeCaps::new();
    let svc = std::sync::Arc::new(todo_core::TodoServiceImpl::new(caps.clone()));
    let app = todo_api::__todo_server::todo_router(svc);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, caps)
}

#[tokio::test]
async fn a_core_call_over_http_reaches_the_generated_route() {
    let (base, caps) = serve().await;

    let created = call(&caps, &base, "/todo/create").await.unwrap();

    assert_eq!(created.title, "buy sandspice");
    assert!(!created.done);
}

#[tokio::test]
async fn a_missing_route_comes_back_as_a_status_the_caller_can_read() {
    let (base, caps) = serve().await;

    let err = call(&caps, &base, "/todo/nope")
        .await
        .expect_err("a call to a missing route must fail");

    assert!(err.to_string().contains("404"), "got: {err}");
}
