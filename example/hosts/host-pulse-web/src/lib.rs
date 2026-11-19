//! pulse web host: both features and the renderer, compiled to wasm32.
//!
//! The capability bundle is `gabion::testing::TestCaps` — a fixed clock, an
//! in-memory store, and a scripted http. That is the same bundle the unit tests
//! run on, and it says what this host is today: a wall proof, not a browser app.
//! A js-clock, an IndexedDB store, and a `fetch`-backed http land with the
//! WebCaps pass.

use checks_api::{CheckId, CheckService, NewCheck};
use checks_core::CheckServiceImpl;
use gabion::testing::TestCaps;
use incidents_api::IncidentService;
use incidents_core::IncidentServiceImpl;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// The two watches the demo board shows, in order.
pub const DEMO_WATCHES: [(&str, &str); 2] = [
    ("api", "http://api.internal/health"),
    ("cdn", "http://cdn.internal/health"),
];

/// Both services, wired over one capability bundle.
pub struct Board {
    pub checks: Arc<dyn CheckService + Send + Sync>,
    pub incidents: Arc<dyn IncidentService + Send + Sync>,
}

fn services(caps: TestCaps) -> Board {
    let checks: Arc<dyn CheckService + Send + Sync> = Arc::new(CheckServiceImpl::new(caps.clone()));
    let incidents: Arc<dyn IncidentService + Send + Sync> = Arc::new(IncidentServiceImpl::new(
        caps.clone(),
        CheckServiceImpl::new(caps),
    ));
    Board { checks, incidents }
}

/// Browser entry: launches the status board with both services injected through
/// the launch context.
#[wasm_bindgen(start)]
pub fn start() {
    let board = services(TestCaps::new());
    dioxus::LaunchBuilder::new()
        .with_context(pulse_render::Svc {
            checks: board.checks,
            incidents: board.incidents,
        })
        .launch(pulse_render::StatusApp);
}

/// Two watches, two sweeps a minute apart, then what the board would show:
/// name, whether the newest probe answered, uptime percent, and whether an
/// incident is open. Every number after the name arrives through the incidents
/// contract, which reads checks — the same path the browser takes.
#[allow(clippy::unwrap_used)] // bootstrap path for the demo board, not business logic
pub async fn board(caps: TestCaps) -> Vec<(String, bool, u8, bool)> {
    let checks = CheckServiceImpl::new(caps.clone());
    let incidents = IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps.clone()));
    for (name, url) in DEMO_WATCHES {
        checks
            .add(NewCheck {
                name: name.into(),
                url: url.into(),
            })
            .await
            .unwrap();
    }
    checks.sweep(()).await.unwrap();
    caps.time().advance(61_000);
    checks.sweep(()).await.unwrap();
    incidents.evaluate(()).await.unwrap();

    let open = incidents.list(()).await.unwrap();
    let mut rows = Vec::new();
    for report in checks.list(()).await.unwrap() {
        let uptime = incidents
            .uptime(CheckId(report.check.id.clone()))
            .await
            .unwrap();
        rows.push((
            report.check.name.clone(),
            report.last.map(|p| p.ok).unwrap_or(false),
            uptime.pct,
            open.iter().any(|i| i.check_id == report.check.id),
        ));
    }
    rows
}
