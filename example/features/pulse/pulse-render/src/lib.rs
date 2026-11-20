//! pulse status UI. It can name `CheckService`, `IncidentService`, and the wire
//! types — and nothing from either core. The host injects both impls, so this
//! crate compiles identically for native and wasm.
//!
//! The screen reads two features and writes to both: watches come from
//! `checks`, uptime and outages from `incidents`, and every button is a call on
//! one of the two contracts. The UI never learns that uptime is computed from
//! probe history, and it never sees a store.
//!
//! Composition is two layers. [`StatusApp`] is the screen: it reads the injected
//! handles from context, loads, and renders app chrome. [`board::BoardPanel`] is
//! the part another feature may mount, and it takes only props — rows, outages and
//! one callback. A crate composing this feature's UI names the panel and the wire
//! types, never `Svc` or the two service traits.
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
use ui_shared::{Empty, Failure, Heading, Notice, Surface};

mod board;
mod time;

use board::{board_body, latency, load};

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
        None => rsx! { Empty { note: "loading…".to_string() } },
        Some(Err(e)) => rsx! { Failure { message: e.clone() } },
        Some(Ok(loaded)) => board_body(loaded, &svc, ui),
    };
    let actions = rsx! {
        button { r#type: "button", class: "primary", onclick: sweep_click, "sweep due" }
        button { r#type: "button", class: "ghost", onclick: evaluate_click, "evaluate" }
        button { r#type: "button", class: "ghost", onclick: refresh_click, "refresh" }
    };

    rsx! {
        style { dangerous_inner_html: ui_shared::CSS }
        Surface {
            Heading {
                title: "pulse".to_string(),
                subtitle: Some("watch URLs, record outages".to_string()),
                actions: Some(actions),
            }
            if !note().is_empty() {
                Notice { message: note() }
            }
            if !error().is_empty() {
                Failure { message: error() }
            }
            {body}
            form { class: "gb add", onsubmit: add,
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
