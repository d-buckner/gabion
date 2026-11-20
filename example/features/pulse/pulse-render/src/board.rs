//! The board's view layer: the rows, the outage log, and the reads behind them.
//!
//! Split from `lib.rs` so the file that owns the app's state and actions stays
//! readable; this module renders what the two contracts returned.
//!
//! [`BoardPanel`] is the part another feature may compose, and it takes *props*:
//! data built from the two contracts' wire types, plus a callback for the one
//! action it offers. It never takes a service handle, so a crate that mounts this
//! panel inherits one function signature — not `Svc`, not both features'
//! internals, and not the store behind them. That is the coupling the
//! `render -> render` edge is meant to buy: components, not reach.

use checks_api::{CheckId, CheckReport};
use dioxus::prelude::*;
use incidents_api::{Incident, Uptime};

use ui_shared::{Action, Empty};

use crate::time::stamp;
use crate::{act, probe_and_evaluate, Svc, Ui};

/// One row of the board, as data. Public because it is a prop: a crate composing
/// this panel builds rows from `checks-api` and `incidents-api` values it already
/// has, and never sees how the board loads them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckRow {
    pub report: CheckReport,
    pub uptime: Option<Uptime>,
    /// The outage currently open on this watch, if any.
    pub open: Option<Incident>,
}

/// A probe the board asked for: which watch, and the label to report it under.
///
/// A named type rather than `(String, String)`: a payload is part of the component
/// contract, and a positional tuple would make it the kind of signature nobody
/// dares change.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProbeRequest {
    pub check_id: CheckId,
    pub name: String,
}

pub(crate) struct Board {
    rows: Vec<CheckRow>,
    incidents: Vec<Incident>,
}

pub(crate) async fn load(svc: &Svc) -> Result<Board, String> {
    let reports = svc.checks.list(&svc.ctx, ()).await.map_err(|e| e.message)?;
    let incidents = svc
        .incidents
        .list(&svc.ctx, ())
        .await
        .map_err(|e| e.message)?;
    let mut rows = Vec::with_capacity(reports.len());
    for report in reports {
        let uptime = svc
            .incidents
            .uptime(&svc.ctx, CheckId(report.check.id.clone()))
            .await
            .ok();
        let open = incidents
            .iter()
            .find(|i| i.closed_at.is_none() && i.check_id == report.check.id)
            .cloned();
        rows.push(CheckRow {
            report,
            uptime,
            open,
        });
    }
    // The store returns watches in id order, which is creation order with a
    // random nonce: alphabetical by name is the only stable order for a human.
    rows.sort_by(|a, b| a.report.check.name.cmp(&b.report.check.name));
    Ok(Board { rows, incidents })
}

/// Latency is measured on the capability clock, which is whole seconds: a probe
/// that beat the deadline shows a status with no duration rather than a fake 0.
pub(crate) fn latency(ms: u64) -> String {
    if ms == 0 {
        String::new()
    } else {
        format!(" in {ms}ms")
    }
}

fn status(row: &CheckRow) -> (&'static str, String) {
    let Some(last) = &row.report.last else {
        return ("idle", "not probed yet".to_string());
    };
    if last.ok {
        return ("ok", format!("{}{}", last.status, latency(last.latency_ms)));
    }
    let note = if last.note.is_empty() {
        format!("status {}", last.status)
    } else {
        last.note.clone()
    };
    ("bad", note)
}

fn uptime_text(uptime: &Option<Uptime>) -> String {
    match uptime {
        Some(u) => format!("{}% of {}", u.pct, u.probes),
        None => "—".to_string(),
    }
}

/// The rows: one watch each, and a probe button that ignores the schedule.
///
/// Takes the callback instead of the service: the row knows what happened, the app
/// knows what to do about it.
fn row_view(row: &CheckRow, on_probe: EventHandler<ProbeRequest>) -> Element {
    let name = row.report.check.name.clone();
    let (class, text) = status(row);
    let seen = match &row.report.last {
        Some(last) => format!("at {}", stamp(last.at)),
        None => "never".to_string(),
    };
    let outage = match &row.open {
        Some(i) => {
            let since = stamp(i.opened_at);
            rsx! { span { class: "bad", "since {since}" } }
        }
        None => rsx! { span { class: "muted", "—" } },
    };
    let request = ProbeRequest {
        check_id: CheckId(row.report.check.id.clone()),
        name: name.clone(),
    };
    let onclick = move |_| on_probe.call(request.clone());

    rsx! {
        tr {
            td { class: "dot {class}", "●" }
            td {
                div { class: "name", "{name}" }
                div { class: "url", "{row.report.check.url}" }
            }
            td {
                div { class: "{class}", "{text}" }
                div { class: "muted", "{seen}" }
            }
            td { class: "pct", "{uptime_text(&row.uptime)}" }
            td { {outage} }
            td { class: "act",
                Action { label: "probe".to_string(), primary: false, onclick }
            }
        }
    }
}

fn incidents_view(incidents: &[Incident]) -> Element {
    if incidents.is_empty() {
        return rsx! { Empty { note: "no outages recorded".to_string() } };
    }
    let items: Vec<Element> = incidents
        .iter()
        .take(12)
        .map(|i| {
            let when = match i.closed_at {
                Some(closed) => format!("{} → {}", stamp(i.opened_at), stamp(closed)),
                None => format!("open since {}", stamp(i.opened_at)),
            };
            let class = if i.closed_at.is_none() {
                "bad"
            } else {
                "muted"
            };
            rsx! {
                li {
                    span { class: "who", "{i.check_name}" }
                    span { class: "{class}", "{i.cause}" }
                    span { class: "when", "{when}" }
                }
            }
        })
        .collect();
    rsx! { ul { class: "outages", for node in items { {node} } } }
}

/// The board as a component: watches, then the outage log.
///
/// Props only — rows and outages as data, and one callback for the one action.
/// `pub(crate)` today because nothing outside composes it yet: publishing a
/// component is a decision with a consumer behind it, and `over-broad-visibility`
/// is what asks for the `pub` the day a second screen mounts this one.
/// A screen that composes this panel supplies both from its own state, which is
/// what makes "two features on one page" a component question rather than a
/// question about whose service handle wins:
///
/// ```ignore
/// rsx! { BoardPanel { rows, incidents, on_probe } }
/// ```
#[component]
pub(crate) fn BoardPanel(
    rows: Vec<CheckRow>,
    incidents: Vec<Incident>,
    on_probe: EventHandler<ProbeRequest>,
) -> Element {
    let rendered: Vec<Element> = rows.iter().map(|r| row_view(r, on_probe)).collect();
    let empty = rows.is_empty();
    rsx! {
        if empty {
            Empty { note: "no watches yet — add one below".to_string() }
        } else {
            table {
                thead {
                    tr {
                        th {}
                        th { "watch" }
                        th { "last probe" }
                        th { "uptime" }
                        th { "incident" }
                        th {}
                    }
                }
                tbody { for node in rendered { {node} } }
            }
        }
        h2 { "outages" }
        {incidents_view(&incidents)}
    }
}

/// The whole board for the one feature that owns this screen: loads, then hands
/// the data to [`BoardPanel`]. App chrome — the toolbar, the add form, the banners
/// — belongs to [`crate::StatusApp`], which is why it is not here.
pub(crate) fn board_body(loaded: &Board, svc: &Svc, ui: Ui) -> Element {
    let probe_svc = svc.clone();
    let on_probe = EventHandler::new(move |request: ProbeRequest| {
        let svc = probe_svc.clone();
        act(
            probe_and_evaluate(svc, request.check_id.0, request.name),
            ui,
        );
    });
    rsx! {
        BoardPanel {
            rows: loaded.rows.clone(),
            incidents: loaded.incidents.clone(),
            on_probe,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // a failing assertion should show the markup
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // a snapshot test reads an env var and writes a file
mod snapshot {
    //! The published panel, rendered to HTML and compared against a committed file.
    //!
    //! Same idea as `todo-render`'s panel snapshot and as `openapi_snapshot.rs` on
    //! the wire: a component can keep its props and change what it shows, so the
    //! markup is the artifact a reviewer reads. Bless with
    //! `GABION_BLESS_UI=1 cargo test -p pulse-render`.

    use super::*;
    use checks_api::Check;
    use incidents_api::Incident;

    fn check(id: &str, name: &str, url: &str) -> Check {
        Check {
            id: id.into(),
            name: name.into(),
            url: url.into(),
            interval_secs: 60,
            timeout_ms: 2_000,
            created_at: 1_700_000_000,
        }
    }

    /// One watch answering, one in an outage: both branch of the row.
    fn rows() -> Vec<CheckRow> {
        vec![
            CheckRow {
                report: CheckReport {
                    check: check("c1", "api", "https://example.com/health"),
                    last: Some(checks_api::Probe {
                        check_id: "c1".into(),
                        at: 1_700_000_100,
                        ok: true,
                        status: 200,
                        latency_ms: 12,
                        attempts: 1,
                        note: String::new(),
                    }),
                },
                uptime: Some(Uptime {
                    check_id: "c1".into(),
                    probes: 4,
                    ok: 3,
                    pct: 75,
                }),
                open: None,
            },
            CheckRow {
                report: CheckReport {
                    check: check("c2", "melted", "http://127.0.0.1:9/"),
                    last: Some(checks_api::Probe {
                        check_id: "c2".into(),
                        at: 1_700_000_200,
                        ok: false,
                        status: 0,
                        latency_ms: 0,
                        attempts: 3,
                        note: "no answer".into(),
                    }),
                },
                uptime: None,
                open: Some(Incident {
                    id: "i1".into(),
                    check_id: "c2".into(),
                    check_name: "melted".into(),
                    opened_at: 1_700_000_150,
                    closed_at: None,
                    cause: "3 attempts, no answer".into(),
                }),
            },
        ]
    }

    #[component]
    fn Fixture() -> Element {
        let incidents = rows()
            .iter()
            .filter_map(|r| r.open.clone())
            .collect::<Vec<Incident>>();
        rsx! {
            BoardPanel {
                rows: rows(),
                incidents,
                on_probe: move |_request: ProbeRequest| {},
            }
        }
    }

    fn path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../.gabion/ui/board-panel.html")
    }

    #[test]
    fn the_panel_markup_matches_the_committed_snapshot() {
        let mut dom = VirtualDom::new(Fixture);
        dom.rebuild_in_place();
        let rendered = dioxus_ssr::render(&dom);

        let path = path();
        if std::env::var("GABION_BLESS_UI").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &rendered).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "no snapshot at {}: {e}. Run `GABION_BLESS_UI=1 cargo test -p pulse-render`, \
                 then read the diff.",
                path.display()
            )
        });
        assert_eq!(
            rendered, committed,
            "the board markup changed. If it should have, re-bless and review the diff."
        );
    }
}
