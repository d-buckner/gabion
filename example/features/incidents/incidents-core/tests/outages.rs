//! Incidents are derived, and this proves the derivation without a `checks`
//! process: the neighbour is a stub that answers the contract and nothing else.
//! That is the wall doing its job — `incidents-core` cannot reach past
//! `checks-api` even in its own tests, because `checks-core` is not a dependency.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::{
    check_err, Check, CheckErr, CheckError, CheckId, CheckReport, CheckService, NewCheck, Probe,
};
use gabion::prelude::{service_impl, Ctx, ErrorKind};
use gabion::testing::TestCaps;
use incidents_api::{Incident, IncidentService};
use incidents_core::IncidentServiceImpl;

const ID: &str = "c1";

struct Stub {
    history: Vec<Probe>,
}

#[service_impl]
impl CheckService for Stub {
    async fn add(&self, _ctx: &Ctx, _input: NewCheck) -> Result<Check, CheckErr> {
        unreachable!("incidents never adds a watch")
    }

    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<CheckReport>, CheckErr> {
        Ok(vec![CheckReport {
            check: Check {
                id: ID.into(),
                name: "api".into(),
                url: "http://api.internal/health".into(),
                interval_secs: 60,
                timeout_ms: 2_000,
                created_at: 0,
            },
            last: self.history.last().cloned(),
        }])
    }

    async fn probe(&self, _ctx: &Ctx, _input: CheckId) -> Result<Probe, CheckErr> {
        unreachable!("incidents never probes")
    }

    async fn sweep(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<Probe>, CheckErr> {
        unreachable!("incidents never sweeps")
    }

    async fn history(&self, _ctx: &Ctx, input: CheckId) -> Result<Vec<Probe>, CheckErr> {
        if input.0 != ID {
            return Err(check_err(CheckError::NotFound));
        }
        Ok(self.history.clone())
    }
}

fn probe(at: u64, ok: bool, note: &str) -> Probe {
    Probe {
        check_id: ID.into(),
        at,
        ok,
        status: if ok { 200 } else { 0 },
        latency_ms: 5,
        attempts: if ok { 1 } else { 3 },
        note: note.into(),
    }
}

fn service(caps: &TestCaps, history: &[Probe]) -> IncidentServiceImpl<TestCaps, Stub> {
    IncidentServiceImpl::new(
        caps.clone(),
        Stub {
            history: history.to_vec(),
        },
    )
}

async fn evaluate(caps: &TestCaps, history: &[Probe]) -> Vec<Incident> {
    service(caps, history)
        .evaluate(&Ctx::anonymous(), ())
        .await
        .unwrap()
}

#[tokio::test]
async fn one_miss_is_not_an_incident() {
    let caps = TestCaps::new();
    let changed = evaluate(
        &caps,
        &[probe(10, true, ""), probe(20, false, "no connection")],
    )
    .await;
    assert!(changed.is_empty(), "one bad probe is noise, not an outage");
}

#[tokio::test]
async fn two_misses_open_one_incident_dated_at_the_first() {
    let caps = TestCaps::new();
    let changed = evaluate(
        &caps,
        &[
            probe(10, true, ""),
            probe(100, false, "no connection"),
            probe(150, false, "no connection"),
        ],
    )
    .await;
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0].opened_at, 100,
        "dated when it broke, not when noticed"
    );
    assert_eq!(changed[0].check_name, "api");
    assert_eq!(changed[0].cause, "no connection");
    assert!(changed[0].closed_at.is_none());
}

/// The store is shared, the neighbour is not: a second service over the same
/// store must not open a second incident for the same outage.
#[tokio::test]
async fn the_same_outage_does_not_open_twice() {
    let caps = TestCaps::new();
    let down = vec![
        probe(100, false, "no connection"),
        probe(150, false, "no connection"),
    ];
    assert_eq!(evaluate(&caps, &down).await.len(), 1);
    assert!(
        evaluate(&caps, &down).await.is_empty(),
        "a second look at the same outage must not double-count it"
    );
    assert_eq!(
        service(&caps, &[])
            .list(&Ctx::anonymous(), ())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn an_incident_closes_when_the_next_probe_answers() {
    let caps = TestCaps::new();
    evaluate(
        &caps,
        &[
            probe(100, false, "no connection"),
            probe(150, false, "no connection"),
        ],
    )
    .await;

    let recovered = [
        probe(100, false, "no connection"),
        probe(150, false, "no connection"),
        probe(200, true, ""),
    ];
    let changed = evaluate(&caps, &recovered).await;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].closed_at, Some(caps.time().now_ms() / 1_000));

    assert!(
        evaluate(&caps, &recovered).await.is_empty(),
        "a closed incident stays closed"
    );
}

#[tokio::test]
async fn open_incidents_come_before_closed_ones() {
    let caps = TestCaps::new();
    evaluate(
        &caps,
        &[
            probe(100, false, "no connection"),
            probe(150, false, "no connection"),
        ],
    )
    .await;
    let all = service(&caps, &[])
        .list(&Ctx::anonymous(), ())
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].closed_at.is_none());
}

#[tokio::test]
async fn uptime_counts_the_window() {
    let caps = TestCaps::new();
    let history = [
        probe(10, true, ""),
        probe(20, false, "no connection"),
        probe(30, true, ""),
        probe(40, true, ""),
    ];
    let uptime = service(&caps, &history)
        .uptime(&Ctx::anonymous(), CheckId(ID.into()))
        .await
        .unwrap();
    assert_eq!(uptime.probes, 4);
    assert_eq!(uptime.ok, 3);
    assert_eq!(uptime.pct, 75);
}

/// No history is not perfect uptime. A watch nobody probed must not read as a
/// healthy one.
#[tokio::test]
async fn a_watch_with_no_history_has_no_uptime() {
    let caps = TestCaps::new();
    let uptime = service(&caps, &[])
        .uptime(&Ctx::anonymous(), CheckId(ID.into()))
        .await
        .unwrap();
    assert_eq!(uptime.pct, 0);
    assert_eq!(uptime.probes, 0);
}

#[tokio::test]
async fn a_neighbour_404_stays_a_404() {
    let caps = TestCaps::new();
    let err = service(&caps, &[])
        .uptime(&Ctx::anonymous(), CheckId("unknown".into()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
}
