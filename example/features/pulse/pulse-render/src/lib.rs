//! pulse status UI. It can name `CheckService`, `IncidentService`, and the wire
//! types — and nothing from either core. The host injects both impls, so this
//! crate compiles identically for native and wasm.
//!
//! The screen reads two features. It gets the watches from `checks` and the
//! uptime and outages from `incidents`, which is the cross-feature call made
//! visible: the UI never learns that uptime is computed from probe history.
//!
//! ```ignore
//! dioxus::LaunchBuilder::new()
//!     .with_context(pulse_render::Svc { checks, incidents })
//!     .launch(pulse_render::StatusApp);
//! ```

use checks_api::{CheckId, CheckReport, CheckService};
use dioxus::prelude::*;
use incidents_api::{Incident, IncidentService, Uptime};
use std::sync::Arc;

const CSS: &str = r#"
body { font-family: ui-sans-serif, system-ui, sans-serif; background: #101014;
       color: #e8e6e3; margin: 0; }
main { max-width: 46rem; margin: 3rem auto; padding: 0 1rem; }
h1 { font-size: 1.4rem; font-weight: 600; letter-spacing: .01em; }
table { width: 100%; border-collapse: collapse; }
td { padding: .55rem .2rem; border-bottom: 1px solid #23232b; }
td.up { width: .8rem; }
.name { font-weight: 600; }
.url { opacity: .55; font-size: .85rem; }
.ok { color: #4ec9b0; }
.bad { color: #e06c75; }
.idle { opacity: .4; }
.pct { text-align: right; font-variant-numeric: tabular-nums; }
.err { color: #e06c75; }
.muted { opacity: .5; font-size: .85rem; }
"#;

/// Service handles injected by the host through the launch context. The UI only
/// ever sees the two contracts.
#[derive(Clone)]
pub struct Svc {
    pub checks: Arc<dyn CheckService + Send + Sync>,
    pub incidents: Arc<dyn IncidentService + Send + Sync>,
}

#[derive(Clone)]
struct Row {
    report: CheckReport,
    uptime: Option<Uptime>,
    open: Option<Incident>,
}

async fn load(svc: Svc) -> Result<Vec<Row>, String> {
    let reports = svc.checks.list(()).await.map_err(|e| e.message)?;
    let incidents = svc.incidents.list(()).await.map_err(|e| e.message)?;
    let mut rows = Vec::with_capacity(reports.len());
    for report in reports {
        let id = report.check.id.clone();
        let uptime = svc.incidents.uptime(CheckId(id)).await.ok();
        let open = incidents
            .iter()
            .find(|i| i.closed_at.is_none() && i.check_id == report.check.id)
            .cloned();
        rows.push(Row {
            report,
            uptime,
            open,
        });
    }
    Ok(rows)
}

fn dot(row: &Row) -> (&'static str, String) {
    let Some(last) = &row.report.last else {
        return ("idle", "not probed yet".to_string());
    };
    if last.ok {
        return ("ok", format!("{} in {}ms", last.status, last.latency_ms));
    }
    let note = if last.note.is_empty() {
        format!("status {}", last.status)
    } else {
        last.note.clone()
    };
    ("bad", note)
}

/// The status board: one row per watch, plus what `incidents` has to say.
#[component]
pub fn StatusApp() -> Element {
    let svc = use_context::<Svc>();
    let rows = use_resource({
        let svc = svc.clone();
        move || load(svc.clone())
    });

    let board = match rows() {
        None => rsx! { p { class: "muted", "loading…" } },
        Some(Err(e)) => rsx! { p { class: "err", "{e}" } },
        Some(Ok(items)) => rsx! {
            if items.is_empty() {
                p { class: "muted", "no watches yet" }
            }
            table {
                for row in items {
                    tr {
                        key: "{row.report.check.id}",
                        td { class: "up {dot(&row).0}", "●" }
                        td {
                            div { class: "name", "{row.report.check.name}" }
                            div { class: "url", "{row.report.check.url}" }
                        }
                        td { "{dot(&row).1}" }
                        td {
                            class: "pct",
                            match &row.uptime {
                                Some(u) => rsx! { "{u.pct}%" },
                                None => rsx! { span { class: "muted", "—" } },
                            }
                        }
                        td {
                            match &row.open {
                                Some(i) => rsx! { span { class: "bad", "outage since {i.opened_at}" } },
                                None => rsx! {},
                            }
                        }
                    }
                }
            }
        },
    };

    rsx! {
        style { dangerous_inner_html: CSS }
        main {
            h1 { "pulse" }
            {board}
        }
    }
}
