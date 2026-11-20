//! pulse desktop host: both cores over one file-backed capability bundle, the
//! render crate in a Dioxus window, and a first-run demo board so the app is
//! something to look at rather than an empty table.
//!
//! Zero business logic here. Hosts are effect homes — reading env, opening a
//! file, and owning the process are exactly what a host is for — and
//! `gabion check` audits that this allow stays in a `host-*` crate.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use checks_api::{CheckService, NewCheck};
use checks_core::CheckServiceImpl;
use gabion::prelude::Ctx;
use gabion_native::FileCaps;
use incidents_api::IncidentService;
use incidents_core::IncidentServiceImpl;
use std::sync::Arc;

/// Two watches on first run: one that answers, one that cannot, so the demo
/// shows a green row, a red row, an open incident, and a 0% uptime.
const DEMO_WATCHES: [(&str, &str); 2] = [
    ("example", "https://example.com"),
    ("offline", "http://127.0.0.1:9/"),
];

fn die(message: String) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

/// Add the demo watches, but only to an empty board: a second launch must not
/// resurrect watches the user never added.
fn seed(checks: &CheckServiceImpl<FileCaps>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => die(format!("cannot start runtime: {e}")),
    };
    runtime.block_on(async {
        let ctx = Ctx::anonymous();
        let empty = checks.list(&ctx, ()).await.map(|items| items.is_empty());
        if !matches!(empty, Ok(true)) {
            return;
        }
        for (name, url) in DEMO_WATCHES {
            if let Err(e) = checks
                .add(
                    &ctx,
                    NewCheck {
                        name: name.into(),
                        url: url.into(),
                    },
                )
                .await
            {
                eprintln!("cannot add demo watch {name}: {}", e.message);
            }
        }
    });
}

fn main() {
    let path = std::env::var("PULSE_STORE").unwrap_or_else(|_| "pulse-gui.json".into());
    let caps = match FileCaps::new(path) {
        Ok(caps) => caps,
        Err(e) => die(format!("cannot open store: {e}")),
    };
    let checks = CheckServiceImpl::new(caps.clone());
    seed(&checks);
    let incidents = IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps));

    let svc = pulse_render::Svc::new(
        Arc::new(checks) as Arc<dyn CheckService + Send + Sync>,
        Arc::new(incidents) as Arc<dyn IncidentService + Send + Sync>,
    );

    // Dioxus owns its async runtime, so the window launches outside one. The
    // capability bundle's timers and http client run on the runtime Dioxus
    // brings up for the event loop.
    dioxus::LaunchBuilder::new()
        .with_context(svc)
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_window(dioxus::desktop::WindowBuilder::new().with_title("pulse")),
        )
        .launch(pulse_render::StatusApp);
}
