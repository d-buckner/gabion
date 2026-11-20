//! The config audits: the wall in `Cargo.toml` is itself audited, so deleting it is
//! a named finding instead of an absence of evidence.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{
    honest_clippy_toml, honest_workspace_lints, member, rules_found, write_ws, ws_head,
    ws_head_with_lints,
};
use gabion_cli::{check, Options};

/// A member that never opts into `[workspace.lints]` gets none of it, so the
/// whole block is inert for that crate.
fn member_no_optin(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n")
}

/// The spec claims `unsafe_code = "forbid"` workspace-wide. A claim with no
/// config behind it is a wall drawn on paper, so a missing block is a finding.
/// Cargo itself rejects a member that opts into `[workspace.lints]` when the
/// workspace never declares one, so the two ways to be missing are always
/// together: no block, and nobody opted in.
#[test]
fn a_workspace_with_no_lints_block_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head_with_lints(&["x-api"], "")),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member_no_optin("x-api")),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let summary = {
        let mut opts = Options::new(tmp.path());
        opts.fast = true;
        check(&opts)
    };
    let lint_findings: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| f.rule == "workspace-lints")
        .map(|f| f.detail.as_str())
        .collect();
    assert!(
        lint_findings
            .iter()
            .any(|d| d.contains("[workspace.lints]")),
        "a workspace with no [workspace.lints] must be named, got {lint_findings:?}"
    );
}

/// `unsafe_code = "warn"` reads as set and blocks nothing.
#[test]
fn a_loosened_workspace_lint_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loosened =
        honest_workspace_lints().replace("unsafe_code = \"forbid\"", "unsafe_code = \"warn\"");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head_with_lints(&["x-api"], &loosened)),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let summary = {
        let mut opts = Options::new(tmp.path());
        opts.fast = true;
        check(&opts)
    };
    let lint_findings: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| f.rule == "workspace-lints")
        .map(|f| f.detail.as_str())
        .collect();
    assert!(
        lint_findings.iter().any(|d| d.contains("unsafe_code")),
        "a loosened unsafe_code must be named, got {lint_findings:?}"
    );
}

/// `[workspace.lints]` does nothing on its own: each member has to opt in.
#[test]
fn a_member_that_skips_the_workspace_lints_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member_no_optin("x-api")),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let found = rules_found(tmp.path());
    assert!(
        found.contains("workspace-lints"),
        "a member that opts out of the workspace lints must be named, got {found:?}"
    );
}

/// A contract crate that re-exports a whole module puts whatever that module
/// exports on the public wall. clippy cannot see this: `wildcard_imports` skips
/// `pub use` globs (measured on rust 1.99), so the wall needs its own rule.
#[test]
fn a_glob_reexport_in_a_contract_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let glob = "\
pub mod inner {
    pub struct A;
}
pub use inner::*;
";
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", glob),
        ],
    );
    let found = rules_found(tmp.path());
    assert!(
        found.contains("contract-glob"),
        "a glob re-export across the contract wall must be named, got {found:?}"
    );

    // Naming what you publish is the whole requirement.
    write_ws(
        tmp.path(),
        &[(
            "x-api/src/lib.rs",
            "pub mod inner {\n    pub struct A;\n}\npub use inner::A;\n",
        )],
    );
    let found = rules_found(tmp.path());
    assert!(
        !found.contains("contract-glob"),
        "a named re-export is not a finding, got {found:?}"
    );
}

/// README: "An app may tighten them through `max-thresholds`. The gate only ever
/// tightens them." A knob that can loosen the audit is the delete-the-wall diff
/// taken in two steps.
#[test]
fn an_app_cannot_loosen_a_required_threshold() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loosened = honest_clippy_toml().replace(
        "cognitive-complexity-threshold = 15",
        "cognitive-complexity-threshold = 999",
    );
    let head = ws_head(&["x-api"]).replace(
        "max-branches = 2",
        "max-branches = 2\nmax-thresholds = { cognitive-complexity-threshold = 999 }",
    );
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &head),
            ("clippy.toml", &loosened),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let summary = {
        let mut opts = Options::new(tmp.path());
        opts.fast = true;
        check(&opts)
    };
    let config_findings: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| f.rule == "clippy-config")
        .map(|f| f.detail.as_str())
        .collect();
    assert!(
        config_findings
            .iter()
            .any(|d| d.contains("cognitive-complexity-threshold")),
        "a threshold loosened through gabion config must still be a finding, got {config_findings:?}"
    );
}
