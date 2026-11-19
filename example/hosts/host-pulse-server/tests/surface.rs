//! The generated HTTP surface for both features, driven over a socket. The
//! caller here is the `Http` capability, which is what a cross-feature call or a
//! generated client would use — no test-only http client.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{Caps, Http, HttpRequest};
use serde_json::{json, Value};

async fn serve() -> (String, gabion_native::NativeCaps) {
    let caps = gabion_native::NativeCaps::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = host_pulse_server::app();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, caps)
}

async fn post(
    caps: &gabion_native::NativeCaps,
    base: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let reply = caps
        .http()
        .send(
            HttpRequest::post(format!("{base}{path}"), serde_json::to_vec(&body).unwrap())
                .with_header("content-type", "application/json"),
        )
        .await
        .unwrap();
    let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    (reply.status, value)
}

#[tokio::test]
async fn a_watch_added_over_http_comes_back_in_the_list() {
    let (base, caps) = serve().await;

    let (status, added) = post(
        &caps,
        &base,
        "/check/add",
        json!({ "name": "api", "url": "http://api.internal/health" }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(added["name"], "api");

    let (status, list) = post(&caps, &base, "/check/list", json!(null)).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["check"]["id"], added["id"]);
}

/// The error envelope is one shape for every failure, so a caller reads a status
/// and a message whatever the feature was.
#[tokio::test]
async fn an_unknown_watch_comes_back_as_404_in_the_error_envelope() {
    let (base, caps) = serve().await;
    // `CheckId` is a newtype, so its JSON is the bare id, not an object.
    let (status, body) = post(&caps, &base, "/check/probe", json!("nope")).await;
    assert_eq!(status, 404);
    assert_eq!(body["kind"], "not_found");
    assert!(body["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn a_malformed_body_is_400_in_the_same_envelope() {
    let (base, caps) = serve().await;
    let reply = caps
        .http()
        .send(HttpRequest::post(
            format!("{base}/check/add"),
            b"{not json".to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 400);
    let body: Value = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(body["kind"], "invalid_input");
}

/// The contract the process serves is the contract the snapshot tests committed.
/// A document nobody can fetch is a document nobody checks.
#[tokio::test]
#[allow(clippy::disallowed_methods)] // reads the committed artifact off disk
async fn the_served_contract_is_the_committed_contract() {
    let (base, caps) = serve().await;
    let reply = caps
        .http()
        .send(HttpRequest::get(format!("{base}/openapi.json")))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    let served: Value = serde_json::from_slice(&reply.body).unwrap();

    for (key, file) in [("checks", "check.json"), ("incidents", "incident.json")] {
        let committed = std::fs::read_to_string(format!(
            "{}/../../../.gabion/openapi/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(
            served[key],
            serde_json::from_str::<Value>(&committed).unwrap(),
            "the served `{key}` document differs from the committed one"
        );
    }
}
