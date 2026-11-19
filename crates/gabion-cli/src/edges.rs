//! The edge table: which crate class may depend on which.
//!
//! This is the process wall and the feature wall. Cargo makes an import
//! impossible only while the dependency is absent from `[dependencies]` — an
//! agent (or a tired human) adds one line to Cargo.toml and the wall is gone.
//! This module is what closes that gap, and each rule name doubles as the
//! documentation the violator gets pointed at.

use crate::config::{Class, Config};
use crate::graph::{Crate, Graph};
use crate::report::Finding;

/// Named rules, so a failure tells you which wall you hit rather than "bad dep".
fn rule_name(from: Class, to: Class) -> &'static str {
    match (from, to) {
        (Class::Render, Class::Core) => "process-wall",
        (Class::Core, Class::Core) => "feature-wall",
        (Class::Api, Class::Core) | (Class::Api, Class::Render) => "contract-purity",
        (Class::Shared, _) => "upward-leak",
        (Class::Framework, Class::Api)
        | (Class::Framework, Class::Core)
        | (Class::Framework, Class::Render)
        | (Class::Framework, Class::Shared) => "framework-purity",
        (_, Class::Host) => "host-dependency",
        (Class::Render, Class::Render) => "render-isolation",
        (Class::Core, Class::Render) => "core-imports-render",
        _ => "edge-table",
    }
}

/// The fix text is the documentation: it names the correct path, not the
/// suppression.
fn fix_for(rule: &'static str) -> &'static str {
    match rule {
        "process-wall" => {
            "the render crate may name the service *trait* (from *-api) only; the host injects the impl. Never depend on *-core."
        }
        "feature-wall" => {
            "talk to the other feature through its *-api crate (and the generated client); its core internals are not yours."
        }
        "contract-purity" => "the contract crate holds wire types and the trait only. Move the type into *-api and let *-core depend on it.",
        "upward-leak" => "shared is below every feature: it may depend on the gabion runtime only.",
        "framework-purity" => "the gabion runtime must stay app-agnostic; move the app-specific code into a feature.",
        "host-dependency" => "hosts are composition roots: nothing may depend on them.",
        "render-isolation" => "one feature's UI must not reach into another's; expose what you need through *-api.",
        "core-imports-render" => "core is business logic; it must not know a UI exists.",
        _ => "see the allowed-edges table in [workspace.metadata.gabion].",
    }
}

#[must_use]
pub fn check(graph: &Graph, cfg: &Config) -> Vec<Finding> {
    let mut findings = Vec::new();
    for krate in &graph.crates {
        if krate.class == Class::Unclassified {
            findings.push(Finding::new(
                "unclassified-crate",
                &krate.name,
                format!("`{}` is a workspace member gabion cannot classify (rel path `{}`)", krate.name, krate.rel_dir),
                "name it *-api | *-core | *-render | *-shared | host-*, or list it in [workspace.metadata.gabion] framework-crates / effect-homes. An unclassified crate slips past the whole edge table.",
            ));
        }
        for dep in &krate.ws_deps {
            let Some(target) = graph.find(dep) else {
                continue;
            };
            if let Some(f) = check_edge(krate, target, cfg) {
                findings.push(f);
            }
        }
    }
    findings
}

fn check_edge(from: &Crate, to: &Crate, cfg: &Config) -> Option<Finding> {
    if cfg.may_depend_on(from.class, to.class) {
        return None;
    }
    let rule = rule_name(from.class, to.class);
    Some(Finding::new(
        rule,
        format!("{} -> {}", from.name, to.name),
        format!(
            "{} ({}) may not depend on {} ({})",
            from.name,
            from.class.key(),
            to.name,
            to.class.key()
        ),
        fix_for(rule),
    ))
}
