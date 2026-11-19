//! Integration tests are effect homes: they build fixture workspaces on disk
//! and unwrap freely. gabion's own source rules do not scan `tests/`, and
//! clippy's workspace bans are lifted here exactly as they are for a host crate.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
//!
//! Negative fixtures: every rule must actually bite.
//!
//! The audit that produced these rules found a README claiming walls that were
//! one line of Cargo.toml away from nothing. A rule with no failing test is the
//! same trap, so each rule below is proven against a fixture that violates it,
//! and a clean fixture is proven to pass.

use std::collections::BTreeSet;
use std::path::Path;

use gabion_cli::config::Config;
use gabion_cli::{check, Options};

/// Write a workspace from a path -> contents map.
fn write_ws(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap())
            .map_err(|e| format!("cannot create {}: {e}", full.display()))
            .unwrap();
        std::fs::write(&full, content)
            .map_err(|e| format!("cannot write {path}: {e}"))
            .unwrap();
    }
}

fn rules_found(root: &Path) -> BTreeSet<&'static str> {
    let mut opts = Options::new(root);
    opts.fast = true;
    let summary = check(&opts);
    summary.findings.iter().map(|f| f.rule).collect()
}

fn member(name: &str, deps: &[&str]) -> String {
    let deps: String = deps
        .iter()
        .map(|d| format!("{d} = {{ path = \"../{d}\" }}\n"))
        .collect();
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
    )
}

fn ws_head(members: &[&str]) -> String {
    let members = members
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\
[workspace]
resolver = \"2\"
members = [{members}]

[workspace.metadata.gabion]
framework-crates = []
effect-homes = []
max-file-lines = 12
max-branches = 2

[workspace.metadata.gabion.edges]
api = [\"framework\", \"shared\", \"api\"]
core = [\"framework\", \"shared\", \"api\"]
render = [\"framework\", \"shared\", \"api\"]
shared = [\"framework\"]
host = [\"framework\", \"shared\", \"api\", \"core\", \"render\"]
framework = [\"framework\"]
unclassified = []
"
    )
}

/// Contract: one fn with no surface attr, one with a bogus surface name.
const X_API_SRC: &str = "\
#[service]
pub trait XService {
    #[surface(cli, bogus)]
    async fn a(&self) -> u8;
    async fn b(&self) -> u8;
    #[internal]
    async fn c(&self) -> u8;
}
";

/// Core: effect-home allow, a body comment, a silent default, a TODO, and a fn
/// over the branch limit.
const X_CORE_SRC: &str = "\
#![allow(clippy::disallowed_types)]
pub fn f(x: Option<u8>) -> u8 {
    // a note inside a service body
    let y = x.unwrap_or_default();
    let z = y > 4 && y < 9;
    if y > 1 {
        1
    } else if y > 2 {
        2
    } else {
        3
    }
}
// TODO: delete this crate
";

/// A workspace violating one of everything gabion can see statically.
#[test]
fn every_rule_bites() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            (
                "Cargo.toml",
                &ws_head(&["x-api", "x-core", "x-render", "host-x", "weird"]),
            ),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", X_CORE_SRC),
            // render: the process wall, crossed
            (
                "x-render/Cargo.toml",
                &member("x-render", &["x-api", "x-core"]),
            ),
            ("x-render/src/lib.rs", "pub fn r() {}\n"),
            // host: business logic in a composition root
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            (
                "host-x/src/lib.rs",
                "\
pub struct Q;
impl x_api::XService for Q {
    async fn a(&self) -> u8 {
        0
    }
}
",
            ),
            // a member gabion cannot classify
            ("weird/Cargo.toml", &member("weird", &[])),
            ("weird/src/lib.rs", "pub fn w() {}\n"),
        ],
    );

    let found = rules_found(tmp.path());
    for rule in [
        "process-wall",
        "unclassified-crate",
        "surface-completeness",
        "surface-unknown",
        "effect-home-allow",
        "no-comments-in-service-bodies",
        "core-silent-default",
        "no-todo",
        "too-many-branches",
        "host-business-logic",
        "file-length",
        "clippy-config",
    ] {
        assert!(
            found.contains(rule),
            "rule `{rule}` did not fire; findings were {found:?}"
        );
    }
}

/// A clippy.toml carrying exactly the bans and thresholds gabion requires.
fn honest_clippy_toml() -> String {
    let cfg = Config::default();
    let mut out = String::new();
    out.push_str("disallowed-types = [\n");
    for ban in cfg
        .required_bans
        .iter()
        .filter(|b| !b.contains("::now") && !b.contains("sleep"))
    {
        out.push_str(&format!("    {{ path = \"{ban}\", reason = \"test\" }},\n"));
    }
    out.push_str("]\ndisallowed-methods = [\n");
    for ban in cfg
        .required_bans
        .iter()
        .filter(|b| b.contains("::now") || b.contains("sleep"))
    {
        out.push_str(&format!("    {{ path = \"{ban}\", reason = \"test\" }},\n"));
    }
    out.push_str("]\n");
    for (key, value) in &cfg.max_thresholds {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// The same workspace, cleaned up: no findings at all.
#[test]
fn clean_workspace_passes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            (
                "Cargo.toml",
                &ws_head(&["x-api", "x-core", "x-render", "host-x"]),
            ),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "\
#[service]
pub trait XService {
    #[surface(cli, http)]
    async fn a(&self) -> u8;
    #[internal]
    async fn c(&self) -> u8;
}
",
            ),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            (
                "x-core/src/lib.rs",
                "pub fn f(x: Option<u8>) -> u8 { x.unwrap_or(0) }\n",
            ),
            ("x-render/Cargo.toml", &member("x-render", &["x-api"])),
            ("x-render/src/lib.rs", "pub fn r() {}\n"),
            (
                "host-x/Cargo.toml",
                &member("host-x", &["x-core", "x-render"]),
            ),
            ("host-x/src/lib.rs", "fn main() {}\n"),
        ],
    );

    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let rules: Vec<&str> = summary.findings.iter().map(|f| f.rule).collect();
    assert!(summary.ok(), "clean fixture failed: {rules:?}");
}

/// Deleting a ban from clippy.toml is a named failure, not silence.
#[test]
fn deleting_a_ban_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            (
                "clippy.toml",
                "disallowed-types = []\ndisallowed-methods = []\n",
            ),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let config_findings: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| f.rule == "clippy-config")
        .map(|f| f.detail.as_str())
        .collect();
    assert!(
        config_findings
            .iter()
            .any(|d| d.contains("std::thread::spawn")),
        "a gutted clippy.toml must be reported, got {config_findings:?}"
    );
}

/// A forbidden edge is reported with the crate pair, so the message is the doc.
#[test]
fn edge_finding_names_the_wall_and_the_fix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "x-render"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", "pub fn f() {}\n"),
            (
                "x-render/Cargo.toml",
                &member("x-render", &["x-api", "x-core"]),
            ),
            ("x-render/src/lib.rs", "pub fn r() {}\n"),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let wall = summary.findings.iter().find(|f| f.rule == "process-wall");
    let Some(wall) = wall else {
        panic!("expected a process-wall finding");
    };
    assert!(
        wall.detail.contains("x-render -> x-core") || wall.location.contains("x-render -> x-core")
    );
    assert!(
        wall.fix.contains("*-api"),
        "the fix should point at the correct path: {}",
        wall.fix
    );
}
