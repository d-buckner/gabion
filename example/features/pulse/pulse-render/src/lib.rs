//! pulse status UI. It can name `CheckService`, `IncidentService`, and the wire
//! types — and nothing from either core. The host injects both impls, so this
//! crate compiles identically for native and wasm.
//!
//! The screen reads two features and writes to both: watches come from
//! `checks`, uptime and outages from `incidents`, and every button is a call on
//! one of the two contracts. The UI never learns that uptime is computed from
//! probe history, and it never sees a store.
//!
//! ```ignore
//! dioxus::LaunchBuilder::new()
//!     .with_context(pulse_render::Svc { checks, incidents })
//!     .launch(pulse_render::StatusApp);
//! ```

use checks_api::{CheckId, CheckService, NewCheck};
use dioxus::prelude::*;
use gabion::prelude::Ctx;
use incidents_api::IncidentService;
use std::future::Future;
use std::sync::Arc;

mod board;
mod time;

use board::{board_view, latency, load};

pub use time::stamp;

const CSS: &str = r#"
:root { color-scheme: dark; }
body { font-family: ui-sans-serif, system-ui, sans-serif; background: #101014;
       color: #e8e6e3; margin: 0; }
main { max-width: 52rem; margin: 2.5rem auto 5rem; padding: 0 1.25rem; }
header { display: flex; align-items: baseline; gap: .75rem; margin-bottom: 1.5rem; }
h1 { font-size: 1.35rem; font-weight: 600; letter-spacing: .02em; margin: 0; }
h2 { font-size: .8rem; font-weight: 600; text-transform: uppercase;
     letter-spacing: .08em; opacity: .55; margin: 2.25rem 0 .5rem; }
.sub { opacity: .45; font-size: .85rem; }
.actions { margin-left: auto; display: flex; gap: .5rem; }
button { font: inherit; font-size: .8rem; color: #e8e6e3; background: #26262f;
         border: 1px solid #33333d; border-radius: .375rem; padding: .3rem .6rem;
         cursor: pointer; }
button:hover { background: #31313c; }
button.primary { background: #3b5bdb; border-color: #4c6ef5; }
button.primary:hover { background: #4c6ef5; }
button.ghost { background: transparent; }
button.ghost:hover { background: #1e1e26; }
form.add { display: flex; gap: .5rem; margin: 1.5rem 0 0; }
input { font: inherit; font-size: .85rem; color: inherit; background: #17171d;
        border: 1px solid #2c2c36; border-radius: .375rem; padding: .4rem .6rem; }
input.name { width: 9rem; }
input.url { flex: 1; }
input:focus { outline: 1px solid #4c6ef5; border-color: #4c6ef5; }
table { width: 100%; border-collapse: collapse; margin-top: 1.75rem; }
th { text-align: left; font-size: .7rem; font-weight: 500; text-transform: uppercase;
     letter-spacing: .07em; opacity: .4; padding: 0 .4rem .5rem; }
td { padding: .6rem .4rem; border-top: 1px solid #23232b; vertical-align: top; }
td.dot { width: .9rem; padding-top: .65rem; }
td.pct { text-align: right; font-variant-numeric: tabular-nums; }
td.act { text-align: right; width: 4.5rem; }
.name { font-weight: 600; }
.url { opacity: .45; font-size: .78rem; word-break: break-all; }
.ok { color: #4ec9b0; }
.bad { color: #e06c75; }
.idle { opacity: .35; }
.muted { opacity: .5; font-size: .8rem; }
.err { color: #e06c75; font-size: .85rem; }
.notice { color: #4ec9b0; font-size: .85rem; }
.banner { margin: .75rem 0 0; }
ul.outages { list-style: none; padding: 0; margin: 0; font-size: .85rem; }
ul.outages li { display: flex; gap: .6rem; padding: .45rem .1rem;
                border-top: 1px solid #23232b; }
ul.outages .who { font-weight: 600; min-width: 7rem; }
ul.outages .when { opacity: .5; margin-left: auto; font-variant-numeric: tabular-nums; }
"#;

/// Service handles injected by the host through the launch context. The UI only
/// ever sees the two contracts.
#[derive(Clone)]
pub struct Svc {
    pub checks: Arc<dyn CheckService + Send + Sync>,
    pub incidents: Arc<dyn IncidentService + Send + Sync>,
    /// The caller this board acts as. Travels with the handle so every button
    /// on the screen makes an identified call, not an anonymous one.
    pub ctx: Ctx,
}

impl Svc {
    #[must_use]
    pub fn new(
        checks: Arc<dyn CheckService + Send + Sync>,
        incidents: Arc<dyn IncidentService + Send + Sync>,
    ) -> Self {
        Self {
            checks,
            incidents,
            ctx: Ctx::anonymous(),
        }
    }
}

/// Signals the actions touch: bump `version` to re-run the loader, and post the
/// outcome of an action into one of the two banners.
#[derive(Clone, Copy)]
pub(crate) struct Ui {
    version: Signal<u32>,
    note: Signal<String>,
    error: Signal<String>,
}

/// Run one action, then re-read the board. `Ok` is shown as a note, `Err` as an
/// error, and only success bumps the version (a failure did not change state).
pub(crate) fn act<F>(fut: F, ui: Ui)
where
    F: Future<Output = Result<String, String>> + 'static,
{
    let mut ui = ui;
    spawn(async move {
        match fut.await {
            Ok(note) => {
                ui.error.set(String::new());
                ui.note.set(note);
                ui.version += 1;
            }
            Err(error) => ui.error.set(error),
        }
    });
}

async fn sweep_and_evaluate(svc: Svc) -> Result<String, String> {
    let probed = svc
        .checks
        .sweep(&svc.ctx, ())
        .await
        .map_err(|e| e.message)?;
    let changed = svc
        .incidents
        .evaluate(&svc.ctx, ())
        .await
        .map_err(|e| e.message)?;
    Ok(format!(
        "probed {}, {} incident change(s)",
        probed.len(),
        changed.len()
    ))
}

pub(crate) async fn probe_and_evaluate(
    svc: Svc,
    id: String,
    name: String,
) -> Result<String, String> {
    let probe = svc
        .checks
        .probe(&svc.ctx, CheckId(id))
        .await
        .map_err(|e| e.message)?;
    let changed = svc
        .incidents
        .evaluate(&svc.ctx, ())
        .await
        .map_err(|e| e.message)?;
    let verdict = if probe.ok {
        format!(
            "{} answered {}{}",
            name,
            probe.status,
            latency(probe.latency_ms)
        )
    } else {
        format!("{} did not answer: {}", name, probe.note)
    };
    Ok(format!("{verdict}; {} incident change(s)", changed.len()))
}

/// The status board: one row per watch, the outage log, and the controls that
/// drive both features.
#[component]
pub fn StatusApp() -> Element {
    let svc = use_context::<Svc>();
    let mut version = use_signal(|| 0u32);
    let mut note = use_signal(String::new);
    let error = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut url = use_signal(String::new);
    let ui = Ui {
        version,
        note,
        error,
    };

    let board = use_resource({
        let svc = svc.clone();
        move || {
            let svc = svc.clone();
            async move {
                let _ = version();
                load(&svc).await
            }
        }
    });

    let sweep_svc = svc.clone();
    let sweep_click = move |_| act(sweep_and_evaluate(sweep_svc.clone()), ui);
    let evaluate_svc = svc.clone();
    let evaluate_click = move |_| {
        let svc = evaluate_svc.clone();
        act(
            async move {
                let changed = svc
                    .incidents
                    .evaluate(&svc.ctx, ())
                    .await
                    .map_err(|e| e.message)?;
                Ok(format!("{} incident change(s)", changed.len()))
            },
            ui,
        );
    };
    let refresh_click = move |_| {
        note.set(String::new());
        version += 1;
    };
    let add_svc = svc.clone();
    let add = move |evt: FormEvent| {
        evt.prevent_default();
        let (form_name, form_url) = (name(), url());
        let svc = add_svc.clone();
        name.set(String::new());
        url.set(String::new());
        act(
            async move {
                svc.checks
                    .add(
                        &svc.ctx,
                        NewCheck {
                            name: form_name.clone(),
                            url: form_url,
                        },
                    )
                    .await
                    .map_err(|e| e.message)?;
                Ok(format!("watching {form_name}"))
            },
            ui,
        );
    };

    let current = board.read();
    let body = match &*current {
        None => rsx! { p { class: "muted", "loading…" } },
        Some(Err(e)) => rsx! { p { class: "err", "{e}" } },
        Some(Ok(loaded)) => board_view(loaded, &svc, ui),
    };

    rsx! {
        style { dangerous_inner_html: CSS }
        main {
            header {
                h1 { "pulse" }
                span { class: "sub", "watch URLs, record outages" }
                div { class: "actions",
                    button { r#type: "button", class: "primary", onclick: sweep_click, "sweep due" }
                    button { r#type: "button", class: "ghost", onclick: evaluate_click, "evaluate" }
                    button { r#type: "button", class: "ghost", onclick: refresh_click, "refresh" }
                }
            }
            if !note().is_empty() {
                p { class: "notice banner", "{note}" }
            }
            if !error().is_empty() {
                p { class: "err banner", "{error}" }
            }
            {body}
            form { class: "add", onsubmit: add,
                input {
                    class: "name",
                    placeholder: "name",
                    value: "{name}",
                    oninput: move |e| name.set(e.value()),
                }
                input {
                    class: "url",
                    placeholder: "https://example.com/health",
                    value: "{url}",
                    oninput: move |e| url.set(e.value()),
                }
                button { r#type: "submit", class: "primary", "watch" }
            }
        }
    }
}
