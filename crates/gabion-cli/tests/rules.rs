//! Source rules and edges: every rule proven against a fixture that violates it.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{
    honest_clippy_toml, member, member_with, rules_found, write_ws, ws_head, X_API_SRC,
};
use gabion_cli::{check, Options};

/// A contract that re-exports a whole module: public, and invisible to clippy.
const X_API_GLOB: &str = "\
pub mod inner {
    pub struct A;
}
pub use inner::*;

#[service]
pub trait XService {
    async fn a(&self) -> u8;
    async fn b(&self) -> u8;
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
            ("x-api/src/lib.rs", X_API_GLOB),
            (
                "x-core/Cargo.toml",
                &member_with("x-core", &["x-api"], "tokio = \"1\"\n"),
            ),
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
        "surface-snapshot",
        "effect-home-allow",
        "no-comments-in-service-bodies",
        "core-silent-default",
        "no-todo",
        "too-many-branches",
        "host-business-logic",
        "file-length",
        "clippy-config",
        "contract-glob",
        "core-runtime-dep",
    ] {
        assert!(
            found.contains(rule),
            "rule `{rule}` did not fire; findings were {found:?}"
        );
    }
}

/// A runtime in a core is a manifest reading, not a build failure: tokio's `rt`
/// feature compiles for wasm32-unknown-unknown, so this rule is the wall. A
/// target-specific dependency is read too, since it is how a native-only branch
/// would sneak past a plain `[dependencies]` scan.
#[test]
fn a_runtime_in_a_core_is_a_finding_but_a_host_may_have_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "host-x"])),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
            (
                "x-core/Cargo.toml",
                &format!(
                    "{}[target.'cfg(unix)'.dependencies]\nreqwest = \"0.12\"\n",
                    member_with("x-core", &["x-api"], "tokio = \"1\"\naxum = \"0.8\"\n")
                ),
            ),
            ("x-core/src/lib.rs", "pub fn f() {}\n"),
            (
                "host-x/Cargo.toml",
                &member_with("host-x", &["x-core"], "tokio = \"1\"\n"),
            ),
            ("host-x/src/lib.rs", "pub fn h() {}\n"),
        ],
    );

    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let hits: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| f.rule == "core-runtime-dep")
        .map(|f| f.location.as_str())
        .collect();

    assert_eq!(
        hits,
        vec!["x-core -> axum", "x-core -> reqwest", "x-core -> tokio"],
        "only the core crate is audited for runtime crates"
    );
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
            // A lock that does not match is written by `--update-surfaces` below,
            // so the fixture starts without one.
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "\
#[service]
pub trait XService {
    async fn a(&self) -> u8;
}

// in-proc only: no surface, so nothing to lock
pub trait XInternals {
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
            // The host names the core's one public item, which is what makes it not
            // over-broad: `pub` in a core is the "a host may name this" boundary.
            (
                "host-x/src/lib.rs",
                "fn main() { let _ = x_core::f(None); }\n",
            ),
        ],
    );

    // The lock carries a hash of each signature, so it is generated rather than
    // written by hand — the same way an app gets one.
    let mut generate = Options::new(tmp.path());
    generate.fast = true;
    generate.update_surfaces = true;
    let _ = check(&generate);

    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let rules: Vec<&str> = summary.findings.iter().map(|f| f.rule).collect();
    eprintln!(
        "LOCK: {:?}",
        std::fs::read_to_string(tmp.path().join(".gabion/surfaces.lock"))
    );
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
