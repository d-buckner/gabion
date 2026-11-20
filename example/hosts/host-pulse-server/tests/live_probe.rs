//! The probe path against a live socket: the real `Http` capability, the real
//! clock, and a core that was written without knowing either exists.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::http::StatusCode;
use axum::routing::get;
use checks_api::{CheckId, CheckService, NewCheck};
use checks_core::CheckServiceImpl;
use gabion::prelude::Ctx;

/// A server with one healthy route and one that is melting.
async fn target() -> String {
    let app = axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/melted",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "melted") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// A port nobody listens on: bound, then released.
async fn dead_port() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    base
}

#[tokio::test]
async fn a_live_route_that_answers_is_recorded_as_up() {
    let ctx = Ctx::anonymous();
    let base = target().await;
    let caps = gabion_native::NativeCaps::new();
    let svc = CheckServiceImpl::new(caps);
    let check = svc
        .add(
            &ctx,
            NewCheck {
                name: "target".into(),
                url: format!("{base}/health"),
            },
        )
        .await
        .unwrap();

    let probe = svc.probe(&ctx, CheckId(check.id)).await.unwrap();
    assert!(probe.ok, "a live 200 must read as up: {:?}", probe);
    assert_eq!(probe.status, 200);
    assert_eq!(probe.attempts, 1);
}

/// A `503` is an answer, so the ladder does not retry it. Retrying an outage
/// would hide it behind three waits.
#[tokio::test]
async fn a_live_route_that_melts_is_recorded_once() {
    let ctx = Ctx::anonymous();
    let base = target().await;
    let caps = gabion_native::NativeCaps::new();
    let svc = CheckServiceImpl::new(caps);
    let check = svc
        .add(
            &ctx,
            NewCheck {
                name: "melted".into(),
                url: format!("{base}/melted"),
            },
        )
        .await
        .unwrap();

    let probe = svc.probe(&ctx, CheckId(check.id)).await.unwrap();
    assert!(!probe.ok);
    assert_eq!(probe.status, 503);
    assert_eq!(probe.attempts, 1);
    assert_eq!(probe.note, "status 503");
}

/// Nothing listening: no response ever arrives, so the ladder runs its three
/// tries and records the miss with the reason.
#[tokio::test]
async fn a_port_nobody_listens_on_is_a_miss_after_the_ladder() {
    let ctx = Ctx::anonymous();
    let base = dead_port().await;
    let caps = gabion_native::NativeCaps::new();
    let svc = CheckServiceImpl::new(caps);
    let check = svc
        .add(
            &ctx,
            NewCheck {
                name: "dead".into(),
                url: base,
            },
        )
        .await
        .unwrap();

    let probe = svc.probe(&ctx, CheckId(check.id)).await.unwrap();
    assert!(!probe.ok);
    assert_eq!(probe.status, 0);
    assert_eq!(probe.attempts, 3);
    assert!(!probe.note.is_empty(), "the miss must say why");
}
