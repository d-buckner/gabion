//! pulse web host: both features and the renderer, compiled to wasm32 and
//! wired to real browser capabilities ([`WebCaps`]).
//!
//! This is the same code path the desktop host runs — the same `checks-core`,
//! the same `incidents-core`, the same `pulse-render` — with the five effects
//! swapped for their js counterparts. State lives in localStorage, so a reload
//! keeps the board; probes go out through `fetch`.
//!
//! Two caveats that come with the browser, not with gabion: fetch is
//! CORS-bound (probe targets must allow this origin), and localStorage is
//! per-origin (the board is private to wherever it is served from).

use checks_api::{CheckService, NewCheck};
use checks_core::CheckServiceImpl;
use gabion::caps::Caps;
use gabion::prelude::Ctx;
use incidents_api::IncidentService;
use incidents_core::IncidentServiceImpl;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

mod bridge;
mod webcaps;
mod ws;

pub use webcaps::WebCaps;
pub use ws::WebStreams;

/// What the demo board starts with on a fresh origin: one CORS-friendly watch
/// that answers, and one port nobody listens on, so the first sweep produces an
/// open incident too.
pub const DEMO_WATCHES: [(&str, &str); 2] = [
    ("api", "https://api.github.com/zen"),
    ("offline", "http://127.0.0.1:9/"),
];

/// Both services, wired over one capability bundle.
pub struct Board {
    pub checks: Arc<dyn CheckService + Send + Sync>,
    pub incidents: Arc<dyn IncidentService + Send + Sync>,
}

fn services<C: Caps>(caps: C) -> Board {
    let checks: Arc<dyn CheckService + Send + Sync> = Arc::new(CheckServiceImpl::new(caps.clone()));
    let incidents: Arc<dyn IncidentService + Send + Sync> = Arc::new(IncidentServiceImpl::new(
        caps.clone(),
        CheckServiceImpl::new(caps),
    ));
    Board { checks, incidents }
}

async fn seed(checks: &dyn CheckService) {
    let ctx = Ctx::anonymous();
    let empty = checks.list(&ctx, ()).await.map(|items| items.is_empty());
    if !matches!(empty, Ok(true)) {
        return;
    }
    for (name, url) in DEMO_WATCHES {
        let _ = checks
            .add(
                &ctx,
                NewCheck {
                    name: name.into(),
                    url: url.into(),
                },
            )
            .await;
    }
}

/// What this page can call, straight from the contracts.
///
/// The registry is an inherent method on the trait object rather than a
/// linker-collected slice, so it works here — a browser build has no linker
/// sections to collect and no composition root to run twice. A board that can say
/// which operations it holds is a board you can audit.
#[wasm_bindgen]
pub fn exposed_ops() -> String {
    let board = services(WebCaps::new());
    let mut host = gabion::registry::Host::new();
    board.checks.mount_on(&mut host);
    board.incidents.mount_on(&mut host);
    host.exposed().join(",")
}

/// Browser entry: seed a fresh origin, then launch the same status board the
/// desktop host shows, with the services injected through the launch context.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        let board = services(WebCaps::new());
        seed(board.checks.as_ref()).await;
        dioxus::LaunchBuilder::new()
            .with_context(pulse_render::Svc::new(board.checks, board.incidents))
            .launch(pulse_render::StatusApp);
    });
}

/// Two watches, two sweeps a minute apart, then what the board would show:
/// name, whether the newest probe answered, uptime percent, and whether an
/// incident is open. Every number after the name arrives through the incidents
/// contract, which reads checks — the same path the browser takes.
#[allow(clippy::unwrap_used)] // bootstrap path for the demo board, not business logic
pub async fn board(caps: gabion::testing::TestCaps) -> Vec<(String, bool, u8, bool)> {
    let ctx = Ctx::anonymous();
    let checks = CheckServiceImpl::new(caps.clone());
    let incidents = IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps.clone()));
    for (name, url) in DEMO_WATCHES {
        checks
            .add(
                &ctx,
                NewCheck {
                    name: name.into(),
                    url: url.into(),
                },
            )
            .await
            .unwrap();
    }
    checks.sweep(&ctx, ()).await.unwrap();
    caps.time().advance(61_000);
    checks.sweep(&ctx, ()).await.unwrap();
    incidents.evaluate(&ctx, ()).await.unwrap();

    let open = incidents.list(&ctx, ()).await.unwrap();
    let mut rows = Vec::new();
    for report in checks.list(&ctx, ()).await.unwrap() {
        let uptime = incidents
            .uptime(&ctx, checks_api::CheckId(report.check.id.clone()))
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
