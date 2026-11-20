//! The rules added to close the gate's own gaps (§8): over-broad visibility,
//! axum outside a host, a workspace with no wasm proof, the files that used to be
//! unscanned, and the lock's signature hash.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{generate_lock, honest_clippy_toml, member, write_ws, ws_head};
use gabion_cli::{check, Options};

fn summary(root: &std::path::Path) -> gabion_cli::report::Summary {
    let mut opts = Options::new(root);
    opts.fast = true;
    check(&opts)
}

/// A `pub` item in a `*-core` that nothing else names is published with no
/// caller: `pub(crate)` is what it meant. `--fix` applies it, and the finding
/// then goes away — the same edit every time, which is the test for whether a
/// rule may have a mechanical fix at all.
#[test]
fn a_pub_nobody_names_is_over_broad_and_the_fix_applies() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            (
                "x-core/src/lib.rs",
                "pub struct Named;\npub fn unused_helper() -> u8 { 0 }\n",
            ),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            // The host names `Named` only: `unused_helper` has no caller.
            (
                "host-x/src/lib.rs",
                "fn main() { let _ = x_core::Named; }\n",
            ),
        ],
    );

    let found = summary(tmp.path());
    let hits: Vec<&str> = found
        .findings
        .iter()
        .filter(|f| f.rule == "over-broad-visibility")
        .map(|f| f.detail.as_str())
        .collect();
    assert_eq!(hits.len(), 1, "exactly the uncalled item: {hits:?}");
    assert!(hits[0].contains("unused_helper"), "{hits:?}");

    let mut fix = Options::new(tmp.path());
    fix.fast = true;
    fix.fix = true;
    let applied = check(&fix);
    assert!(
        applied.failed_steps.is_empty(),
        "the fix must not fail: {:?}",
        applied.failed_steps
    );
    let source = std::fs::read_to_string(tmp.path().join("x-core/src/lib.rs")).unwrap();
    assert!(
        source.contains("pub(crate) fn unused_helper"),
        "the fix must narrow the item:\n{source}"
    );
    assert!(
        source.contains("pub struct Named"),
        "an item a host names must be left alone:\n{source}"
    );

    let after = summary(tmp.path());
    assert!(
        !after
            .findings
            .iter()
            .any(|f| f.rule == "over-broad-visibility"),
        "the fix must clear the finding: {:?}",
        after.findings.iter().map(|f| f.rule).collect::<Vec<_>>()
    );
}

/// A hand-written axum handler in a feature crate bypasses the contract: the
/// route exists, and the OpenAPI document, the MCP tool and the client do not
/// know about it. Hosts may wire endpoints; features may not.
#[test]
fn axum_outside_a_host_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "use axum::Router;\npub fn routes() -> Router { Router::new() }\n",
            ),
            // A fully qualified path, with no import to catch it first.
            (
                "x-api/src/handlers.rs",
                "pub fn h() { let _ = axum::Router::new(); }\n",
            ),
            ("host-x/Cargo.toml", &member("host-x", &["x-api"])),
            (
                "host-x/src/lib.rs",
                "use axum::Router;\npub fn app() -> Router { Router::new() }\n",
            ),
        ],
    );

    let found = summary(tmp.path());
    let hits: Vec<&str> = found
        .findings
        .iter()
        .filter(|f| f.rule == "axum-outside-host")
        .map(|f| f.location.as_str())
        .collect();
    assert!(
        hits.iter().all(|h| h.starts_with("x-api")),
        "only the feature crate is a violation, the host is composition: {hits:?}"
    );
    assert_eq!(
        hits.len(),
        2,
        "both the import and the inline path: {hits:?}"
    );
}

/// The wasm step builds every `*-core`. A workspace with none has nothing to
/// build, so the step passes and proves nothing — the vacuous green §8 names.
#[test]
fn a_workspace_with_no_core_cannot_prove_the_wasm_wall() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-api"])),
            ("host-x/src/lib.rs", "fn main() {}\n"),
        ],
    );
    let found = summary(tmp.path());
    let hit = found
        .findings
        .iter()
        .find(|f| f.rule == "no-wasm-proof")
        .expect("a workspace with no core must say so");
    assert!(hit.fix.contains("feature add"), "{}", hit.fix);
}

/// `tests/` used to be unscanned, which is where an agent writes the most code.
/// The shape rules apply there; the structural ones do not, because an integration
/// test is an effect home and ships nowhere.
#[test]
fn tests_are_scanned_for_shape_but_granted_their_allows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", "pub fn f() {}\n"),
            (
                "x-core/tests/probe.rs",
                "#![allow(clippy::unwrap_used)]\n\n// TODO: this test is a sketch\n#[test]\nfn t() {}\n",
            ),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            ("host-x/src/lib.rs", "fn main() { let _ = x_core::f(); }\n"),
        ],
    );
    let found = summary(tmp.path());
    let rules: Vec<&str> = found.findings.iter().map(|f| f.rule).collect();
    assert!(
        rules.contains(&"no-todo"),
        "a TODO in a test is still a TODO: {rules:?}"
    );
    assert!(
        !rules.contains(&"allow-allowlist"),
        "an integration test is an effect home: {rules:?}"
    );
}

/// A method that keeps its name and changes its input type used to pass the lock.
/// The hash closes that: the line changes, so the diff shows it.
#[test]
fn a_changed_input_type_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "#[service]\npub trait XService {\n    async fn a(&self) -> u8;\n}\n",
            ),
        ],
    );
    generate_lock(tmp.path());

    // Same name, same op count, wider return type.
    write_ws(
        tmp.path(),
        &[(
            "x-api/src/lib.rs",
            "#[service]\npub trait XService {\n    async fn a(&self) -> u16;\n}\n",
        )],
    );
    let found = summary(tmp.path());
    let drift = found
        .findings
        .iter()
        .find(|f| f.rule == "surface-snapshot")
        .expect("a changed signature must drift the lock");
    assert!(
        drift.fix.contains("changed input/output"),
        "the finding must say what the hash is for: {}",
        drift.fix
    );
}
