//! The shared UI kit: design tokens and the shapes every panel shares.
//!
//! Two feature UIs that compose on one screen should look like one app. Before this
//! crate each render crate carried its own stylesheet and its own idea of "empty"
//! and "failed", which is how a composed screen ends up with two palettes and three
//! kinds of grey. The kit is the smallest thing that fixes that: the tokens, the
//! page surface, and the states every panel has (empty, failed, loading).
//!
//! Deliberately not a component framework. It holds what two panels would otherwise
//! duplicate, and nothing else — a kit that grows a table, a form, and a router is a
//! renderer, and the framework's promise is that the renderer is pluggable.
//!
//! It depends on `dioxus` and on nothing else: a `*-shared` crate may use the
//! framework and third-party crates, never a feature's contract.

use dioxus::prelude::*;

/// Design tokens and the base stylesheet, injected once by the app shell.
///
/// Plain CSS as one string rather than a build-step stylesheet because the render
/// crates compile for both native and wasm with no bundler in between: what the
/// desktop window applies is exactly what the page applies.
pub const CSS: &str = r#"
:root {
  color-scheme: dark;
  --gb-bg: #101014;
  --gb-panel: #17171d;
  --gb-border: #23232b;
  --gb-border-strong: #2c2c34;
  --gb-text: #e8e6e3;
  --gb-dim: #8a8f98;
  --gb-accent: #4c6ef5;
  --gb-ok: #4ec9b0;
  --gb-bad: #e06c75;
  --gb-mono: ui-monospace, SFMono-Regular, monospace;
}
* { box-sizing: border-box; }
body { font-family: ui-sans-serif, system-ui, sans-serif; background: var(--gb-bg);
       color: var(--gb-text); margin: 0; }
main.gb { max-width: 52rem; margin: 2.5rem auto 5rem; padding: 0 1.25rem; }
header.gb { display: flex; align-items: baseline; gap: .75rem; margin-bottom: 1.5rem; }
h1 { font-size: 1.35rem; font-weight: 600; letter-spacing: .02em; margin: 0; }
h2 { font-size: .8rem; font-weight: 600; text-transform: uppercase;
     letter-spacing: .08em; opacity: .55; margin: 2.25rem 0 .5rem; }
.sub { opacity: .45; font-size: .85rem; }
.actions { margin-left: auto; display: flex; gap: .5rem; }
button { font: inherit; font-size: .8rem; color: var(--gb-text); background: #26262f;
         border: 1px solid var(--gb-border-strong); border-radius: .375rem;
         padding: .3rem .6rem; cursor: pointer; }
button:hover { background: #31313c; }
button.primary { background: #3b5bdb; border-color: var(--gb-accent); }
button.primary:hover { background: var(--gb-accent); }
button.ghost { background: transparent; }
button.ghost:hover { background: #1e1e26; }
form.gb { display: flex; gap: .5rem; margin: 1.5rem 0 0; }
input { font: inherit; font-size: .85rem; color: inherit; background: var(--gb-panel);
        border: 1px solid var(--gb-border-strong); border-radius: .375rem;
        padding: .4rem .6rem; }
input:focus { outline: 1px solid var(--gb-accent); border-color: var(--gb-accent); }
table { width: 100%; border-collapse: collapse; margin-top: 1.75rem; }
th { text-align: left; font-size: .7rem; font-weight: 500; text-transform: uppercase;
     letter-spacing: .07em; opacity: .4; padding: 0 .4rem .5rem; }
td { padding: .6rem .4rem; border-top: 1px solid var(--gb-border); vertical-align: top; }
ul.gb { list-style: none; padding: 0; margin: 0; }
.gb-row { display: flex; align-items: center; gap: .6rem; padding: .5rem .2rem;
          border-top: 1px solid var(--gb-border); }
.gb-row.done span { text-decoration: line-through; opacity: .5; }
.muted { opacity: .5; font-size: .85rem; }
.err { color: var(--gb-bad); font-size: .85rem; }
.notice { color: var(--gb-ok); font-size: .85rem; }
.ok { color: var(--gb-ok); }
.bad { color: var(--gb-bad); }
.idle { opacity: .35; }
"#;

/// The page surface: one place that decides the width, margin and padding of a
/// screen, so two features on one page cannot disagree about them.
#[component]
pub fn Surface(children: Element) -> Element {
    rsx! { main { class: "gb", {children} } }
}

/// A screen header: title, optional subtitle, and the actions on the right.
#[component]
pub fn Heading(title: String, subtitle: Option<String>, actions: Option<Element>) -> Element {
    rsx! {
        header { class: "gb",
            h1 { "{title}" }
            if let Some(subtitle) = subtitle {
                span { class: "sub", "{subtitle}" }
            }
            if let Some(actions) = actions {
                div { class: "actions", {actions} }
            }
        }
    }
}

/// A button. `primary` for the one action a panel leads with.
#[component]
pub fn Action(label: String, primary: bool, onclick: EventHandler<MouseEvent>) -> Element {
    let class = if primary { "primary" } else { "ghost" };
    rsx! {
        button { r#type: "button", class: "{class}", onclick: move |e| onclick.call(e), "{label}" }
    }
}

/// "Nothing here yet" — every panel has one, and they should read alike.
#[component]
pub fn Empty(note: String) -> Element {
    rsx! { p { class: "muted", "{note}" } }
}

/// A failure the panel can show but not fix: the message the service sent.
#[component]
pub fn Failure(message: String) -> Element {
    rsx! { p { class: "err", "{message}" } }
}

/// A success note: what the last action did.
#[component]
pub fn Notice(message: String) -> Element {
    rsx! { p { class: "notice", "{message}" } }
}
