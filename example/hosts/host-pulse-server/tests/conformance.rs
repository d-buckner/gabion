//! The four-adapter agreement suite, against a running server (spec §8).
//!
//! One case list, four surfaces: the impl called in-process, the generated clap
//! tree, the generated MCP tool, and the generated client over a real socket.
//! They must answer identically — including on failure, where the envelope, the
//! kind and the detail are all compared.
//!
//! What this cannot cover, and the suite says so rather than quietly skipping it:
//! a *create* is not repeatable. Two calls to `check.add` are two watches with two
//! ids, so comparing them would be comparing two different calls. The cases are
//! therefore observations and failures — the ones a second identical call answers
//! the same way — and a create-shaped op is covered by the client tests, which
//! script the reply instead of repeating the call.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::{CheckId, CheckService};
use gabion::conformance::Case;
use incidents_api::IncidentService;
use serde_json::json;
use std::sync::Arc;

/// Serve the exact impls the suite will also call in-process. Two instances would
/// share a store but not a clock, so the comparison would be between two worlds.
async fn serve(
    checks: Arc<dyn CheckService + Send + Sync>,
    incidents: Arc<dyn IncidentService + Send + Sync>,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = host_pulse_server::app_from((checks, incidents));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

#[tokio::test]
async fn all_four_surfaces_agree_on_observations() {
    let (checks, incidents) = host_pulse_server::services();
    let base = serve(checks.clone(), incidents.clone()).await;

    // One suite per contract: the generated suite only knows its own ops, which
    // is also why passing a neighbour's op is a rejection rather than a pass.
    let check_cases = [Case::new("check.list", json!({}))];
    let checks_report = checks_api::check_conformance(
        checks,
        gabion_native::ReqwestHttp::new(),
        &base,
        &check_cases,
    )
    .await;
    let incident_cases = [
        Case::new("incident.list", json!({})),
        Case::new("incident.evaluate", json!({})),
    ];
    let incidents_report = incidents_api::incident_conformance(
        incidents,
        gabion_native::ReqwestHttp::new(),
        &base,
        &incident_cases,
    )
    .await;

    assert!(checks_report.is_clean(), "{}", checks_report.summary());
    assert!(
        incidents_report.is_clean(),
        "{}",
        incidents_report.summary()
    );
    assert_eq!(
        checks_report.ran() + incidents_report.ran(),
        3,
        "every case ran"
    );
}

/// The interesting half: a failure has to agree too. The route's status, the
/// CLI's exit code and the MCP error code are all derived from one kind, and the
/// flat `WireError` — with the app's own detail in it — has to reach all four.
#[tokio::test]
async fn all_four_surfaces_agree_on_failures() {
    let (checks, incidents) = host_pulse_server::services();
    let base = serve(checks.clone(), incidents.clone()).await;
    let cases = [
        Case::new("check.history", json!("no-such-watch")),
        Case::new("check.probe", json!("no-such-watch")),
        Case::new("incident.uptime", json!("no-such-watch")),
    ];
    let (check_cases, incident_cases) = cases.split_at(2);

    let mut report = checks_api::check_conformance(
        checks,
        gabion_native::ReqwestHttp::new(),
        &base,
        check_cases,
    )
    .await;
    let incidents_report = incidents_api::incident_conformance(
        incidents,
        gabion_native::ReqwestHttp::new(),
        &base,
        incident_cases,
    )
    .await;
    assert!(
        incidents_report.is_clean(),
        "{}",
        incidents_report.summary()
    );
    for agreement in &incidents_report.agreements {
        report.record(agreement.clone());
    }
    assert!(report.is_clean(), "{}", report.summary());

    // And the failure really was a failure on every surface: an agreement on
    // "not found" is only meaningful if none of the four answered success.
    for agreement in &report.agreements {
        assert_eq!(
            agreement.inproc["kind"], "not_found",
            "{} did not fail with not_found: {}",
            agreement.op, agreement.inproc
        );
        assert_eq!(agreement.inproc["detail"], "NotFound");
        assert_eq!(agreement.cli, agreement.inproc);
        assert_eq!(agreement.mcp, agreement.inproc);
        assert_eq!(agreement.http, agreement.inproc);
    }
    let _ = CheckId("no-such-watch".into());
}

/// An op the suite cannot drive is a rejection, not a silent skip: a typo in the
/// case list must fail the suite rather than shorten it.
#[tokio::test]
async fn a_case_no_adapter_implements_is_a_failure() {
    let (checks, incidents) = host_pulse_server::services();
    let base = serve(checks.clone(), incidents.clone()).await;
    let cases = [Case::new("check.nope", json!({}))];

    let report =
        checks_api::check_conformance(checks, gabion_native::ReqwestHttp::new(), &base, &cases)
            .await;
    let _ = incidents;
    assert!(!report.is_clean());
    assert!(
        report.summary().contains("check.nope"),
        "{}",
        report.summary()
    );
}
