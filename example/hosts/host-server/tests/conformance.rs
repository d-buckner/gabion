//! The four-adapter agreement suite for the todo contract.
//!
//! Same shape as the pulse host's, one service: the impl in-process, the
//! generated clap tree, the generated MCP tool, and the generated client over a
//! real socket must answer identically. Every case here is repeatable — an
//! observation, or a failure — because a create-shaped success returns a new id
//! each time and two calls are two different calls.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use gabion::conformance::Case;
use serde_json::json;
use todo_api::TodoService;
use todo_core::TodoServiceImpl;

async fn serve(svc: Arc<dyn TodoService + Send + Sync>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = todo_api::__todo_server::todo_router(svc);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

#[tokio::test]
async fn all_four_surfaces_agree() {
    let svc: Arc<dyn TodoService + Send + Sync> =
        Arc::new(TodoServiceImpl::new(gabion::testing::TestCaps::new()));
    let base = serve(svc.clone()).await;

    let cases = [
        // A success shape: an empty list, from all four.
        Case::new("todo.list", json!({})),
        // A failure the app maps itself: NotFound, with the app's own detail.
        Case::new("todo.complete", json!("no-such-todo")),
        // An input the impl rejects: repeatable, so the four can be compared.
        Case::new("todo.create", json!({"title": "   "})),
    ];

    let report =
        todo_api::todo_conformance(svc, gabion_native::ReqwestHttp::new(), &base, &cases).await;
    assert!(report.is_clean(), "{}", report.summary());
    assert_eq!(report.ran(), 3, "every case ran");

    let missing = report
        .agreements
        .iter()
        .find(|a| a.op == "todo.complete")
        .unwrap();
    assert_eq!(missing.inproc["kind"], "not_found");
    let invalid = report
        .agreements
        .iter()
        .find(|a| a.op == "todo.create")
        .unwrap();
    assert_eq!(
        invalid.inproc["kind"], "invalid_input",
        "the error kind the impl chose must survive all four surfaces: {}",
        invalid.inproc
    );
    assert_eq!(invalid.mcp, invalid.inproc, "MCP agreed on the failure");
    assert_eq!(invalid.cli, invalid.inproc, "the CLI agreed on the failure");
}
