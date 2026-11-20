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
            "talk to the other feature through its *-api crate; the in-process call works today and the generated HTTP client is planned. Its core internals are not yours."
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
        findings.extend(core_runtime_deps(krate));
    }
    findings
}

/// External crates a `*-core` may not depend on.
///
/// The compile target does not seal these. `std` ships wasm stubs for `fs`,
/// `net`, `thread` and `time` (they compile and fail at runtime), and tokio's
/// `rt` feature builds for `wasm32-unknown-unknown` — both verified on the
/// pinned toolchain. So a core that names a runtime, a server, or an I/O client
/// would pass the wasm step, and the only mechanical block left is a manifest
/// read. The list is deliberately enumerated; a crate outside it is not caught
/// (docs/GAPS.md §3).
const CORE_BANNED_CRATES: &[&str] = &[
    // runtimes and executors
    "tokio",
    "tokio-util",
    "async-std",
    "smol",
    "async-executor",
    "rayon",
    "mio",
    // servers and their plumbing
    "axum",
    "actix-web",
    "actix-rt",
    "warp",
    "rocket",
    "poem",
    "tide",
    "hyper",
    "hyper-util",
    "tower",
    "tower-http",
    // clients
    "reqwest",
    "ureq",
    "surf",
    // storage engines
    "rusqlite",
    "sqlx",
    "diesel",
    "redb",
    "sled",
];

/// `[dependencies]` plus every `[target.*.dependencies]` table. Dev-dependencies
/// are not scanned: they are tests, and tests are effect homes. A target-specific
/// dependency is scanned because a core may not grow a native-only branch.
fn core_runtime_deps(krate: &Crate) -> Vec<Finding> {
    if krate.class != Class::Core {
        return Vec::new();
    }
    let manifest = krate.dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return vec![Finding::new(
            "core-runtime-dep",
            krate.name.clone(),
            format!("cannot read {}", manifest.display()),
            "the runtime-dependency audit reads this manifest; fix the path or the file.",
        )];
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return vec![Finding::new(
            "core-runtime-dep",
            krate.name.clone(),
            format!("{} does not parse", manifest.display()),
            "the runtime-dependency audit reads this manifest; fix the syntax.",
        )];
    };
    let mut names = dependency_names(&value);
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter(|name| CORE_BANNED_CRATES.contains(&name.as_str()))
        .map(|name| {
            Finding::new(
                "core-runtime-dep",
                format!("{} -> {name}", krate.name),
                format!("`{}` (core) depends on `{name}`", krate.name),
                "a core is runtime-free: the runtime, server, or client belongs in the host, or behind a capability impl. The wasm target does not catch this one.",
            )
        })
        .collect()
}

fn dependency_names(value: &toml::Value) -> Vec<&String> {
    let mut out: Vec<&String> = Vec::new();
    if let Some(deps) = value.get("dependencies").and_then(toml::Value::as_table) {
        out.extend(deps.keys());
    }
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        for spec in targets.values() {
            if let Some(deps) = spec.get("dependencies").and_then(toml::Value::as_table) {
                out.extend(deps.keys());
            }
        }
    }
    out
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
