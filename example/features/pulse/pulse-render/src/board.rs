//! The board itself: the rows, the outage log, and the reads behind them.
//!
//! Split from `lib.rs` so the file that owns the app's state and actions stays
//! readable; this module only renders what the two contracts returned.

use checks_api::{CheckId, CheckReport};
use dioxus::prelude::*;
use incidents_api::{Incident, Uptime};

use crate::time::stamp;
use crate::{act, probe_and_evaluate, Svc, Ui};

pub(crate) struct Row {
    report: CheckReport,
    uptime: Option<Uptime>,
    open: Option<Incident>,
}

pub(crate) struct Board {
    rows: Vec<Row>,
    incidents: Vec<Incident>,
}

pub(crate) async fn load(svc: &Svc) -> Result<Board, String> {
    let reports = svc.checks.list(()).await.map_err(|e| e.message)?;
    let incidents = svc.incidents.list(()).await.map_err(|e| e.message)?;
    let mut rows = Vec::with_capacity(reports.len());
    for report in reports {
        let uptime = svc
            .incidents
            .uptime(CheckId(report.check.id.clone()))
            .await
            .ok();
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

fn status(row: &Row) -> (&'static str, String) {
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
fn row_view(row: &Row, svc: &Svc, ui: Ui) -> Element {
    let id = row.report.check.id.clone();
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
    let probe_svc = svc.clone();
    let probe_name = name.clone();
    let onclick = move |_| {
        let svc = probe_svc.clone();
        let id = id.clone();
        let name = probe_name.clone();
        act(probe_and_evaluate(svc, id, name), ui);
    };

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
                button { r#type: "button", class: "ghost", onclick, "probe" }
            }
        }
    }
}

fn incidents_view(incidents: &[Incident]) -> Element {
    if incidents.is_empty() {
        return rsx! { p { class: "muted", "no outages recorded" } };
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

/// The board: watches, then the outage log, then the form that adds a watch.
pub(crate) fn board_view(board: &Board, svc: &Svc, ui: Ui) -> Element {
    let rows: Vec<Element> = board.rows.iter().map(|r| row_view(r, svc, ui)).collect();
    let empty = board.rows.is_empty();
    rsx! {
        if empty {
            p { class: "muted", "no watches yet — add one below" }
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
                tbody { for node in rows { {node} } }
            }
        }
        h2 { "outages" }
        {incidents_view(&board.incidents)}
    }
}
