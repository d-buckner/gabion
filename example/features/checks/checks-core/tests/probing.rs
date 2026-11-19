//! Probing is where the capability wall earns its keep: the same code that
//! retries against a socket in a host retries against a script here, and the
//! backoff waits are real waits on a clock that only moves when the code waits.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::{CheckService, NewCheck};
use checks_core::CheckServiceImpl;
use gabion::prelude::{HttpMethod, HttpResponse};
use gabion::testing::{MockHttp, TestCaps};

const URL: &str = "http://api.internal/health";

fn ok() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: b"{}".to_vec(),
    }
}

fn service(reply: Option<HttpResponse>) -> (TestCaps, CheckServiceImpl<TestCaps>) {
    let http = MockHttp::new();
    let http = match reply {
        Some(reply) => http.route(HttpMethod::Get, URL, reply),
        None => http,
    };
    let caps = TestCaps::new().with_http(http);
    let svc = CheckServiceImpl::new(caps.clone());
    (caps, svc)
}

async fn watch(svc: &CheckServiceImpl<TestCaps>) -> String {
    svc.add(NewCheck {
        name: "api".into(),
        url: URL.into(),
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn a_url_that_is_not_http_is_invalid_input() {
    let (_, svc) = service(None);
    let err = svc
        .add(NewCheck {
            name: "api".into(),
            url: "ftp://api.internal/health".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind.http_status(), 400);
}

#[tokio::test]
async fn an_answer_is_recorded_and_shows_up_as_the_latest() {
    let (caps, svc) = service(Some(ok()));
    let id = watch(&svc).await;
    let probe = svc.probe(checks_api::CheckId(id.clone())).await.unwrap();
    assert!(probe.ok);
    assert_eq!(probe.status, 200);
    assert_eq!(probe.attempts, 1, "an answer must not be retried");
    assert!(probe.note.is_empty());

    let sent = caps.http().sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url, URL);

    let reports = svc.list(()).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].last.clone().unwrap().status, 200);
    assert_eq!(svc.history(checks_api::CheckId(id)).await.unwrap().len(), 1);
}

/// A `503` is an answer. Retrying it would hide the outage behind a wait.
#[tokio::test]
async fn an_error_status_is_an_observed_outage_not_a_retry() {
    let (_, svc) = service(Some(HttpResponse {
        status: 503,
        headers: Vec::new(),
        body: Vec::new(),
    }));
    let id = watch(&svc).await;
    let probe = svc.probe(checks_api::CheckId(id)).await.unwrap();
    assert!(!probe.ok);
    assert_eq!(probe.status, 503);
    assert_eq!(probe.attempts, 1);
    assert_eq!(probe.note, "status 503");
}

/// No scripted reply means no connection. The ladder is three tries with a
/// doubling wait, and the virtual clock proves the waits happened.
#[tokio::test]
async fn no_answer_retries_three_times_with_a_doubling_wait() {
    let (caps, svc) = service(None);
    let id = watch(&svc).await;
    let before = caps.time().now_ms();
    let probe = svc.probe(checks_api::CheckId(id)).await.unwrap();
    assert!(!probe.ok);
    assert_eq!(
        probe.status, 0,
        "no response arrived, so there is no status"
    );
    assert_eq!(probe.attempts, 3);
    assert_eq!(probe.note, "no connection");
    assert_eq!(
        caps.time().now_ms() - before,
        750,
        "backoff must be 250ms then 500ms"
    );
    assert_eq!(
        caps.http().sent().len(),
        3,
        "every attempt must actually call out"
    );
}

#[tokio::test]
async fn a_probe_of_an_unknown_watch_is_404() {
    let (_, svc) = service(Some(ok()));
    let err = svc
        .probe(checks_api::CheckId("nope".into()))
        .await
        .unwrap_err();
    assert_eq!(err.kind.http_status(), 404);
    let err = svc
        .history(checks_api::CheckId("nope".into()))
        .await
        .unwrap_err();
    assert_eq!(err.kind.http_status(), 404);
}

#[tokio::test]
async fn sweep_probes_only_what_is_due() {
    let (caps, svc) = service(Some(ok()));
    watch(&svc).await;
    assert_eq!(svc.sweep(()).await.unwrap().len(), 1, "never probed is due");

    caps.time().advance(10_000);
    assert!(
        svc.sweep(()).await.unwrap().is_empty(),
        "a watch probed 10s ago must be left alone"
    );

    caps.time().advance(50_000);
    assert_eq!(
        svc.sweep(()).await.unwrap().len(),
        1,
        "60s since the last probe is due"
    );
    let id = svc.list(()).await.unwrap()[0].check.id.clone();
    assert_eq!(svc.history(checks_api::CheckId(id)).await.unwrap().len(), 2);
}

/// The clock is frozen at one second, so every probe lands on the same key
/// prefix. Without the nonce in the key, the store would hold one record and
/// history would be a lie.
#[tokio::test]
async fn history_keeps_every_probe_taken_in_the_same_second() {
    let (_, svc) = service(Some(ok()));
    let id = watch(&svc).await;
    for _ in 0..5 {
        svc.probe(checks_api::CheckId(id.clone())).await.unwrap();
    }
    assert_eq!(svc.history(checks_api::CheckId(id)).await.unwrap().len(), 5);
}

/// A watch that has been up for a year must not grow the store forever.
#[tokio::test]
async fn history_is_trimmed_to_the_newest_fifty() {
    let (caps, svc) = service(Some(ok()));
    let id = watch(&svc).await;
    for _ in 0..60 {
        caps.time().advance(1_000);
        svc.probe(checks_api::CheckId(id.clone())).await.unwrap();
    }
    let history = svc.history(checks_api::CheckId(id)).await.unwrap();
    assert_eq!(history.len(), 50);
    assert_eq!(history.last().unwrap().at, caps.time().now_ms() / 1_000);
    assert_eq!(history.first().unwrap().at, 1_700_000_011);
}
