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
        gabion_native::WsStreams,
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
        gabion_native::WsStreams,
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

    let report = checks_api::check_conformance(
        checks,
        gabion_native::ReqwestHttp::new(),
        gabion_native::WsStreams,
        &base,
        &cases,
    )
    .await;
    let _ = incidents;
    assert!(!report.is_clean());
    assert!(
        report.summary().contains("check.nope"),
        "{}",
        report.summary()
    );
}

/// A listen op is a case like any other (§9 `conformance` row): items, how the
/// stream ended, and the same comparison across all four adapters.
///
/// Both policies are driven, because they are different promises and a suite that
/// drove only one would leave the other untested while the op list looked complete.
///
/// The watch is created and probed before the suite runs, so the history every
/// surface streams is already fixed. Streaming a watch that the suite also created
/// would not be repeatable, and four surfaces comparing four different calls
/// proves nothing.
#[tokio::test]
async fn every_stream_op_is_driven_through_every_built_adapter() {
    let (checks, incidents) = host_pulse_server::services();
    let base = serve(checks.clone(), incidents.clone()).await;
    let ctx = gabion::Ctx::anonymous().with_principal("conformance");
    let created = checks
        .add(
            &ctx,
            checks_api::NewCheck {
                name: "streamed".to_string(),
                url: "http://127.0.0.1:1/never".to_string(),
            },
        )
        .await
        .unwrap();
    checks
        .probe(&ctx, CheckId(created.id.clone()))
        .await
        .unwrap();

    let cases = [
        Case::new("check.watch", json!(&created.id)),
        Case::new("check.tail", json!(&created.id)),
    ];
    let report = checks_api::check_conformance(
        checks,
        gabion_native::ReqwestHttp::new(),
        gabion_native::WsStreams,
        &base,
        &cases,
    )
    .await;
    assert_eq!(
        report.ran(),
        2,
        "both listen ops must be driven: {}",
        report.summary()
    );
    assert!(report.is_clean(), "{}", report.summary());
    for agreement in &report.agreements {
        // Each surface delivered items and named how its stream ended. A surface
        // that answered with one value instead of a list would fail here, and that
        // is the failure mode §9 exists to catch.
        assert!(
            agreement.inproc["items"].is_array(),
            "{} did not compare an item list: {}",
            agreement.op,
            agreement.inproc
        );
        assert!(
            !agreement.inproc["items"].as_array().unwrap().is_empty(),
            "{} streamed nothing: {}",
            agreement.op,
            agreement.inproc
        );
        assert_eq!(
            agreement.inproc["end"], "done",
            "{}: {}",
            agreement.op, agreement.inproc
        );
        assert_eq!(agreement.cli, agreement.inproc, "{} cli", agreement.op);
        assert_eq!(agreement.mcp, agreement.inproc, "{} mcp", agreement.op);
        assert_eq!(agreement.http, agreement.inproc, "{} http", agreement.op);
    }
}

/// A listen op that fails before its first item is a case like any other.
///
/// The MCP surface cannot answer a stream op with a bare value, so a refusal has
/// to arrive as an empty item list that ended in the error on every surface.
/// Before that normalisation the in-process, CLI and HTTP surfaces answered with
/// the bare error and MCP wrapped it, and the suite reported a disagreement for
/// an op that had behaved correctly.
#[tokio::test]
async fn a_listen_op_that_fails_at_open_agrees() {
    let (checks, incidents) = host_pulse_server::services();
    let base = serve(checks.clone(), incidents.clone()).await;

    let cases = [Case::new("check.watch", json!("no-such-watch"))];
    let report = checks_api::check_conformance(
        checks,
        gabion_native::ReqwestHttp::new(),
        gabion_native::WsStreams,
        &base,
        &cases,
    )
    .await;
    assert!(report.is_clean(), "{}", report.summary());

    let agreement = report.agreements.first().unwrap();
    assert!(
        agreement.inproc["items"].as_array().unwrap().is_empty(),
        "a stream that never opened delivered items: {}",
        agreement.inproc
    );
    assert_eq!(
        agreement.inproc["end"]["kind"], "not_found",
        "the error the impl chose must survive as the end: {}",
        agreement.inproc
    );
}
