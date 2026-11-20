//! `framework-ui`: the framework ships no UI.
//!
//! A design system belongs to the app. gabion generates surfaces, not looks — no
//! components, no tokens, no stylesheet, no default theme. The moment the runtime
//! carries a `<button class="primary">`, every app made with the framework inherits
//! a house style, and the framework has an opinion it never signed up to defend.
//!
//! This is a wall rather than a guideline because it is exactly the kind of thing
//! that arrives one helpful commit at a time: "a sensible default theme", then
//! "just a couple of components", then a design system with no owner. The app-owned
//! side is `*-shared` (`render -> shared` is allowed, `shared` may use the framework
//! only), so an app that wants a kit has a crate for it — the examples ship one
//! (`example/shared/ui-shared`) and it is theirs, not ours.
//!
//! A text scan, not an AST pass: `rsx!` is a macro whose body would need expanding,
//! and a CSS string is a string. The check is deliberately blunt, and the fix is
//! always "move it into the app's `*-shared` crate".
//!
//! Limits: it reads source text, so a mention inside a *doc comment* about UI would
//! count, and it cannot see markup that arrives through another crate's macro. Both
//! are the error direction that gets noticed (a false finding in the framework's own
//! repo), not the silent one.

use crate::config::Class;
use crate::graph::Crate;
use crate::report::Finding;

const RULE: &str = "framework-ui";

/// Markers that mean "this crate is drawing something".
const MARKUP: [&str; 4] = [
    "rsx!",
    "dangerous_inner_html",
    "font-family",
    "color-scheme",
];

/// Findings for a framework crate that has started to render.
#[must_use]
pub fn check(krate: &Crate) -> Vec<Finding> {
    if krate.class != Class::Framework {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for path in krate.source_files() {
        // The rule's own source lists the markers it looks for, so it would find
        // itself. Skipping exactly this file is narrower than exempting the crate,
        // and the four findings it produced on the first run are the proof the scan
        // sees real text.
        if path.ends_with("src/lint/ui.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for marker in MARKUP {
            if !text.contains(marker) {
                continue;
            }
            findings.push(Finding::new(
                RULE,
                krate.at(&path, 1),
                format!("`{marker}` appears in the framework crate `{}`", krate.name),
                "the framework ships no UI: no components, no tokens, no stylesheet, no default \
                 theme. A look in the runtime is a house style every app inherits. Move it into \
                 the app's `*-shared` design-system crate, which `render` and hosts may use.",
            ));
        }
    }
    findings
}
