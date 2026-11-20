//! The capabilities a core needs to wait and to call another service, proven
//! against the deterministic impls. gabion's own source rules do not scan
//! `tests/`; the effect ban is lifted here for the one wall-clock assertion.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::prelude::*;
use gabion::testing::{MockHttp, TestCaps};
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_wait_moves_the_virtual_clock_and_costs_no_wall_time() {
    let caps = TestCaps::new();
    let start_ms = caps.time().now_ms();
    let start = Instant::now();
    for backoff in [100u64, 200, 400] {
        caps.sleep().sleep(backoff).await;
    }
    let wall = start.elapsed();

    assert_eq!(
        caps.time().now_ms() - start_ms,
        700,
        "a retry or debounce loop must be able to read the time it waited"
    );
    assert!(
        wall < Duration::from_millis(50),
        "virtual waits must not cost wall time, took {wall:?}"
    );
}

#[tokio::test]
async fn timeout_cuts_off_a_wait_that_runs_past_the_deadline() {
    let caps = TestCaps::new();
    let sleeper = caps.sleep().clone();

    let outcome = timeout(
        caps.sleep(),
        100,
        Box::pin(async move {
            sleeper.sleep(500).await;
            7
        }),
    )
    .await;

    assert_eq!(outcome, Err(TimedOut));
}

#[tokio::test]
async fn timeout_keeps_a_result_that_arrives_first() {
    let caps = TestCaps::new();
    let sleeper = caps.sleep().clone();

    let outcome = timeout(
        caps.sleep(),
        100,
        Box::pin(async move {
            sleeper.sleep(50).await;
            7
        }),
    )
    .await;

    assert_eq!(outcome, Ok(7));
}

#[tokio::test]
async fn mock_http_replays_the_script_and_records_the_request() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        "http://todo.internal/api/todo.create",
        HttpResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: br#"{"id":"t1"}"#.to_vec(),
        },
    );

    let reply = http
        .send(
            HttpRequest::post(
                "http://todo.internal/api/todo.create",
                br#"{"title":"milk"}"#.to_vec(),
            )
            .with_header("content-type", "application/json"),
        )
        .await
        .unwrap();

    assert!(reply.is_success());
    assert_eq!(reply.body, br#"{"id":"t1"}"#.to_vec());
    let sent = http.sent();
    assert_eq!(sent.len(), 1, "the call must be visible to the test");
    assert_eq!(sent[0].body, br#"{"title":"milk"}"#.to_vec());
    assert_eq!(
        sent[0].headers,
        vec![("content-type".to_string(), "application/json".to_string())]
    );
}

#[tokio::test]
async fn an_unscripted_call_is_an_error_not_an_empty_reply() {
    let http = MockHttp::new();
    let err = http
        .send(HttpRequest::get("http://nowhere.internal/anything"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, HttpError::Transport(_)),
        "a call nobody scripted must fail loudly: {err:?}"
    );
}
