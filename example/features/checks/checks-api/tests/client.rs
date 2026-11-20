//! The generated client for the checks contract: one success, one failure, and
//! the route it chose. The todo contract's `client.rs` covers the mapping rules
//! in depth; this proves the generator produced the right paths and types for a
//! second service with different input and output shapes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::{Check, CheckClient, CheckError, CheckId, CheckService, NewCheck};
use gabion::caps::{HttpMethod, HttpResponse};
use gabion::prelude::{Ctx, ErrorKind};
use gabion::testing::MockHttp;

const BASE: &str = "http://pulse.internal";

fn reply(status: u16, body: &serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn check() -> serde_json::Value {
    serde_json::json!({
        "id": "c1",
        "name": "api",
        "url": "https://example.com/health",
        "interval_secs": 60,
        "timeout_ms": 2000,
        "created_at": 1_700_000_000u64
    })
}

#[tokio::test]
async fn the_client_posts_to_the_check_routes() {
    let http = MockHttp::new()
        .route(
            HttpMethod::Post,
            &format!("{BASE}/api/check.add"),
            reply(200, &check()),
        )
        .route(
            HttpMethod::Post,
            &format!("{BASE}/api/check.probe"),
            reply(
                200,
                &serde_json::json!({
                    "check_id": "c1", "at": 1, "ok": true, "status": 200,
                    "latency_ms": 3, "attempts": 1, "note": ""
                }),
            ),
        );
    let client: &(dyn CheckService + Send + Sync) = &CheckClient::new(BASE, http.clone());
    let ctx = Ctx::anonymous();

    let added = client
        .add(
            &ctx,
            NewCheck {
                name: "api".into(),
                url: "https://example.com/health".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(added.id, "c1");
    let probe = client.probe(&ctx, CheckId("c1".into())).await.unwrap();
    assert!(probe.ok);

    let urls: Vec<String> = http.sent().into_iter().map(|r| r.url).collect();
    assert_eq!(
        urls,
        vec![
            format!("{BASE}/api/check.add"),
            format!("{BASE}/api/check.probe")
        ]
    );
}

#[tokio::test]
async fn a_missing_check_comes_back_as_not_found() {
    let wire = gabion::prelude::WireError::new(ErrorKind::NotFound, "check not found");
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/check.history"),
        reply(404, &serde_json::to_value(&wire).unwrap()),
    );
    let err = CheckClient::new(BASE, http)
        .history(&Ctx::anonymous(), CheckId("nope".into()))
        .await
        .expect_err("a 404 is a failure");
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(matches!(err.detail, CheckError::Remote { .. }));
}

#[tokio::test]
async fn a_check_type_round_trips_through_the_client() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/check.list"),
        reply(
            200,
            &serde_json::json!([{ "check": check(), "last": null }]),
        ),
    );
    let reports = CheckClient::new(BASE, http)
        .list(&Ctx::anonymous(), ())
        .await
        .unwrap();
    let _: &Check = &reports[0].check;
    assert_eq!(reports[0].check.name, "api");
    assert!(reports[0].last.is_none());
}
