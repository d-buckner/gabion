//! The generated typed client, driven against the wire protocol the generated
//! route reads.
//!
//! Nothing here touches a socket: `MockHttp` is a capability, so the client sees
//! exactly what a browser's `fetch` would return and the test can read back what
//! it sent. The two things worth proving are the path/header contract and the
//! error recovery — a client that talks to the wrong route, or that turns a
//! server's "conflict" into "internal", is worse than no client.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{HttpError, HttpMethod, HttpResponse};
use gabion::prelude::{Ctx, ErrorKind};
use gabion::testing::MockHttp;
use todo_api::{CreateTodo, Todo, TodoClient, TodoError, TodoService};

const BASE: &str = "http://todo.internal";

fn reply(status: u16, body: &serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn todo() -> serde_json::Value {
    serde_json::json!({
        "id": "t1",
        "title": "milk",
        "done": false,
        "created_at": 1_700_000_000u64
    })
}

/// The client is a `TodoService`: a host hands it to the renderer in place of the
/// in-process impl, which is the whole point of generating it.
#[tokio::test]
async fn the_client_is_the_service_trait_over_http() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.create"),
        reply(200, &todo()),
    );
    let client: &(dyn TodoService + Send + Sync) = &TodoClient::new(BASE, http);
    let created = client
        .create(
            &Ctx::anonymous(),
            CreateTodo {
                title: "milk".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(created.title, "milk");
}

#[tokio::test]
async fn a_call_posts_to_the_generated_route_with_the_encoded_input() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.create"),
        reply(200, &todo()),
    );
    let client = TodoClient::new(BASE, http.clone());
    client
        .create(
            &Ctx::anonymous(),
            CreateTodo {
                title: "milk".into(),
            },
        )
        .await
        .unwrap();

    let sent = http.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url, format!("{BASE}/api/todo.create"));
    assert_eq!(sent[0].method, HttpMethod::Post);
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["title"], "milk");
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(n, v)| n == "content-type" && v == "application/json"),
        "the route parses the body as JSON: {:?}",
        sent[0].headers
    );
}

/// The identity the caller set is the identity the route will read, because both
/// come from `Ctx::wire_headers`.
#[tokio::test]
async fn the_caller_travels_as_headers() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.list"),
        reply(200, &serde_json::json!([])),
    );
    let client = TodoClient::new(BASE, http.clone());
    let ctx = Ctx::anonymous()
        .with_principal("agent-7")
        .with_tenant("acme");
    client.list(&ctx, ()).await.unwrap();

    let headers = http.sent()[0].headers.clone();
    assert!(headers
        .iter()
        .any(|(n, v)| n == "x-gabion-principal" && v == "agent-7"));
    assert!(headers
        .iter()
        .any(|(n, v)| n == "x-gabion-tenant" && v == "acme"));
}

/// A server that says "conflict" is a conflict, whatever status a proxy put on
/// the reply. The kind comes from the body, and the app's own enum carries it.
#[tokio::test]
async fn a_failure_comes_back_as_the_apps_own_error_type() {
    let wire = gabion::prelude::WireError::new(ErrorKind::Conflict, "already done");
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.complete"),
        reply(409, &serde_json::to_value(&wire).unwrap()),
    );
    let client = TodoClient::new(BASE, http);
    let err = client
        .complete(&Ctx::anonymous(), todo_api::TodoId("t1".into()))
        .await
        .expect_err("a 409 is a failure");

    assert_eq!(err.kind, ErrorKind::Conflict);
    assert_eq!(err.kind.http_status(), 409);
    assert_eq!(err.kind.exit_code(), 4);
    match err.detail {
        TodoError::Remote { kind, message } => {
            assert_eq!(kind, ErrorKind::Conflict);
            assert_eq!(message, "already done");
        }
        other => panic!("expected the remote fallback, got {other:?}"),
    }
}

/// A known detail is recovered whole, so a client that shares the contract's
/// version keeps the typed error rather than a flattened one.
#[tokio::test]
async fn a_known_detail_is_recovered_typed() {
    let typed: todo_api::TodoErr = todo_api::todo_err(TodoError::EmptyTitle);
    let wire = gabion::prelude::WireError::from_service(&typed).unwrap();
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.create"),
        reply(400, &serde_json::to_value(&wire).unwrap()),
    );
    let err = TodoClient::new(BASE, http)
        .create(
            &Ctx::anonymous(),
            CreateTodo {
                title: String::new(),
            },
        )
        .await
        .expect_err("a 400 is a failure");
    assert_eq!(err.detail, TodoError::EmptyTitle);
}

/// A request that never left is not a 404 and not a success: it is internal, and
/// the transport's own words are kept so an operator can act on them.
#[tokio::test]
async fn a_transport_failure_is_internal_and_says_why() {
    let http = MockHttp::new();
    let err = TodoClient::new(BASE, http)
        .list(&Ctx::anonymous(), ())
        .await
        .expect_err("an unscripted route is a transport failure");
    assert_eq!(err.kind, ErrorKind::Internal);
    assert!(
        err.message.contains("no scripted response"),
        "the reason was dropped: {}",
        err.message
    );
    let _: &HttpError = &HttpError::Transport(String::new());
}

/// A base with a trailing slash must not produce `//api/...`: a doubled slash is
/// a 404 on some proxies and a mystery in every bug report.
#[tokio::test]
async fn a_trailing_slash_in_the_base_is_trimmed() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.list"),
        reply(200, &serde_json::json!([])),
    );
    let client = TodoClient::new(format!("{BASE}/"), http.clone());
    client.list(&Ctx::anonymous(), ()).await.unwrap();
    assert_eq!(http.sent()[0].url, format!("{BASE}/api/todo.list"));
}

/// The output type is checked at the boundary: a body of the wrong shape is a
/// failure, not a silently empty list.
#[tokio::test]
async fn a_reply_of_the_wrong_shape_fails() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/todo.list"),
        reply(200, &serde_json::json!({"not": "a list"})),
    );
    let err: gabion::prelude::ServiceError<TodoError> = TodoClient::new(BASE, http)
        .list(&Ctx::anonymous(), ())
        .await
        .expect_err("the body is not a list");
    assert_eq!(err.kind, ErrorKind::Internal);
    let _: Vec<Todo> = Vec::new();
}
