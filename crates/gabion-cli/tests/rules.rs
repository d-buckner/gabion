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

/// Contract: two public ops. Both reach every surface — that is the design,
/// and the surface lock is what makes that visible in review.
const X_API_SRC: &str = "\
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
        "surface-snapshot",
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
            (".gabion/surfaces.lock", "x-api::XService::a\n"),
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

/// Adding a public op is an API change: with no per-method opt-out, it is
/// instantly callable by every caller, including agents. The lock makes that
/// show up in a diff instead of in production.
#[test]
fn surface_drift_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", "x-api::XService::a\n"),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let drift = summary
        .findings
        .iter()
        .find(|f| f.rule == "surface-snapshot")
        .expect("a new public op must be reported");
    assert!(
        drift.detail.contains("+ reachable: x-api::XService::b"),
        "the finding must name the new op, got: {}",
        drift.detail
    );
    assert!(
        !drift.detail.contains("XService::a"),
        "unchanged ops must not be reported: {}",
        drift.detail
    );
}

/// The lock is the security artifact, so it must not depend on spelling. An app
/// that writes `#[gabion::service]` gets the same generated surfaces, and every
/// op in it has to show up in the diff.
#[test]
fn surface_lock_sees_a_qualified_service_attribute() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "\
#[gabion::service]
pub trait XService {
    async fn a(&self) -> u8;
}
",
            ),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let drift = summary
        .findings
        .iter()
        .find(|f| f.rule == "surface-snapshot")
        .expect("an op published through #[gabion::service] must reach the lock");
    assert!(
        drift.detail.contains("x-api::XService::a"),
        "the finding must name the op, got: {}",
        drift.detail
    );
}

/// A `#[service]` trait generates routes and subcommands wherever it is declared.
/// Outside `*-api` the lock used to miss it entirely, so the op was public and
/// unreviewed at the same time.
#[test]
fn service_trait_outside_the_contract_crate_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            (
                "Cargo.toml",
                &ws_head(&["x-api", "x-core", "x-render", "host-x"]),
            ),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            (
                "x-core/src/lib.rs",
                "\
#[service]
pub trait SneakyService {
    async fn op(&self) -> u8;
}
",
            ),
            ("x-render/Cargo.toml", &member("x-render", &["x-api"])),
            ("x-render/src/lib.rs", "pub fn r() {}\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            ("host-x/src/lib.rs", "fn main() {}\n"),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    let placement = summary
        .findings
        .iter()
        .find(|f| f.rule == "service-outside-api")
        .expect("a #[service] trait in a core crate generates surfaces; it must be named");
    assert!(
        placement.detail.contains("SneakyService"),
        "the finding must name the trait, got: {}",
        placement.detail
    );
    let drift = summary
        .findings
        .iter()
        .find(|f| f.rule == "surface-snapshot")
        .expect("the op is reachable, so the lock must show it");
    assert!(
        drift.detail.contains("x-core::SneakyService::op"),
        "the lock must name the op, got: {}",
        drift.detail
    );
}

/// A file `syn` cannot parse is invisible to every source rule, including the
/// surface collector. The lock is only honest if that case is a finding.
#[test]
fn an_unparsable_source_file_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "this is not rust, and it hides a service\n",
            ),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    assert!(
        summary.findings.iter().any(|f| f.rule == "unparsed-source"),
        "an unparsable contract file must be reported, not skipped"
    );
}

/// A workspace with no members makes every rule vacuously green. That is the
/// worst result the gate can produce: it passes and means nothing.
#[test]
fn a_workspace_with_no_members_is_a_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&[])),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    let summary = check(&opts);
    assert!(
        !summary.ok(),
        "a gate that inspected no crate must not report PASS"
    );
}

/// The "yes, I meant to publish that" gesture, and the lock then holds.
#[test]
fn updating_the_lock_clears_the_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
        ],
    );
    let mut opts = Options::new(tmp.path());
    opts.fast = true;
    assert!(
        !check(&opts).ok(),
        "an unlocked surface must fail before it is accepted"
    );

    opts.update_surfaces = true;
    let updated = check(&opts);
    let rules: Vec<&str> = updated.findings.iter().map(|f| f.rule).collect();
    assert!(updated.ok(), "updating the lock must not fail: {rules:?}");
    assert!(tmp.path().join(".gabion/surfaces.lock").is_file());

    opts.update_surfaces = false;
    assert!(
        check(&opts).ok(),
        "the lock just written must satisfy the check"
    );
}
