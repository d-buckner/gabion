//! The generated client for the incidents contract. This one takes a type from
//! *another* contract (`CheckId` from `checks-api`), so it is also the proof that
//! a client method can carry a neighbour's wire type without the client knowing
//! anything about the neighbour's core.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::CheckId;
use gabion::caps::{HttpMethod, HttpResponse};
use gabion::prelude::{Ctx, ErrorKind};
use gabion::testing::MockHttp;
use incidents_api::{IncidentClient, IncidentService};

const BASE: &str = "http://pulse.internal";

fn reply(status: u16, body: &serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: serde_json::to_vec(body).unwrap(),
    }
}

#[tokio::test]
async fn uptime_is_a_post_to_the_incident_route() {
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/incident.uptime"),
        reply(
            200,
            &serde_json::json!({ "check_id": "c1", "probes": 4, "ok": 3, "pct": 75 }),
        ),
    );
    let client: &(dyn IncidentService + Send + Sync) = &IncidentClient::new(BASE, http.clone());
    let uptime = client
        .uptime(&Ctx::anonymous(), CheckId("c1".into()))
        .await
        .unwrap();
    assert_eq!(uptime.pct, 75);
    let sent = http.sent();
    assert_eq!(sent[0].url, format!("{BASE}/api/incident.uptime"));
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!("c1"),
        "CheckId is a newtype over a string"
    );
}

#[tokio::test]
async fn an_evaluate_failure_maps_to_the_kind() {
    let wire = gabion::prelude::WireError::new(ErrorKind::Internal, "storage failed");
    let http = MockHttp::new().route(
        HttpMethod::Post,
        &format!("{BASE}/api/incident.evaluate"),
        reply(500, &serde_json::to_value(&wire).unwrap()),
    );
    let err = IncidentClient::new(BASE, http)
        .evaluate(&Ctx::anonymous(), ())
        .await
        .expect_err("a 500 is a failure");
    assert_eq!(err.kind, ErrorKind::Internal);
    assert_eq!(err.kind.exit_code(), 1);
}
