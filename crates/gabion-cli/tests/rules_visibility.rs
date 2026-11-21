//! The rules added to close the gate's own gaps (§8): over-broad visibility,
//! axum outside a host, a workspace with no wasm proof, the files that used to be
//! unscanned, and the lock's signature hash.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{honest_clippy_toml, member, write_ws, ws_head};
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

/// `render -> render` is allowed, because composing two features on one screen is
/// the normal case — and the visibility rule is what keeps the edge from meaning
/// "reach into another feature's private components". Here the shell render crate
/// composes both feature renders, and every component it names is `pub` in the
/// crate that owns it.
#[test]
fn a_render_crate_may_compose_another_feature_render() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            (
                "Cargo.toml",
                &ws_head(&["todo-api", "todo-core", "todo-render", "billing-api", "billing-render", "app-render", "host-app"]),
            ),
            ("clippy.toml", &honest_clippy_toml()),
            ("todo-api/Cargo.toml", &member("todo-api", &[])),
            ("todo-api/src/lib.rs", "#[service]\npub trait TodoService {\n    async fn a(&self) -> u8;\n}\n"),
            ("todo-core/Cargo.toml", &member("todo-core", &["todo-api"])),
            ("todo-core/src/lib.rs", "pub struct TodoImpl;\n"),
            (
                "todo-render/Cargo.toml",
                &member("todo-render", &["todo-api"]),
            ),
            ("todo-render/src/lib.rs", "pub fn TodoPanel() -> u8 {\n    1\n}\n"),
            ("billing-api/Cargo.toml", &member("billing-api", &[])),
            ("billing-render/Cargo.toml", &member("billing-render", &["billing-api"])),
            ("billing-render/src/lib.rs", "pub fn BillingPanel() -> u8 {\n    2\n}\n"),
            // The composition root: two features on one screen.
            (
                "app-render/Cargo.toml",
                &member("app-render", &["todo-render", "billing-render"]),
            ),
            (
                "app-render/src/lib.rs",
                "pub fn App() -> u8 {\n    todo_render::TodoPanel() + billing_render::BillingPanel()\n}\n",
            ),
            (
                "host-app/Cargo.toml",
                &member("host-app", &["todo-core", "app-render"]),
            ),
            (
                "host-app/src/lib.rs",
                "fn main() {\n    let _ = todo_core::TodoImpl;\n    let _ = app_render::App();\n}\nfn m(s: &dyn todo_api::TodoService) {\n    let _ = s;\n}\n",
            ),
        ],
    );

    let found = summary(tmp.path());
    let rules: Vec<&str> = found.findings.iter().map(|f| f.rule).collect();
    assert!(
        !rules.contains(&"edge-table") && !rules.contains(&"process-wall"),
        "composing two feature UIs is not a violation: {rules:?}"
    );
    assert!(
        !rules.contains(&"over-broad-visibility"),
        "every named component is published, and nothing else is `pub`: {rules:?}"
    );
}

/// The other half of that edge: a render crate's private helper must not be `pub`,
/// or "compose another feature's UI" turns into "reach into its internals".
#[test]
fn a_render_internal_marked_pub_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            (
                "Cargo.toml",
                &ws_head(&["x-api", "x-render", "app-render", "host-app"]),
            ),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-render/Cargo.toml", &member("x-render", &["x-api"])),
            (
                "x-render/src/lib.rs",
                "pub fn Panel() -> u8 {\n    helper()\n}\npub fn helper() -> u8 {\n    0\n}\n",
            ),
            (
                "app-render/Cargo.toml",
                &member("app-render", &["x-render"]),
            ),
            (
                "app-render/src/lib.rs",
                "pub fn App() -> u8 {\n    x_render::Panel()\n}\n",
            ),
            // The composition root is launched by a host, so its entry point is
            // named — otherwise it would be a `pub` nobody uses, which is a finding
            // for the same reason a helper is.
            ("host-app/Cargo.toml", &member("host-app", &["app-render"])),
            (
                "host-app/src/lib.rs",
                "fn main() {\n    let _ = app_render::App();\n}\n",
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
    assert_eq!(hits.len(), 1, "exactly the private helper: {hits:?}");
    assert!(hits[0].contains("helper"), "{hits:?}");
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

/// The framework ships no UI: a design system belongs to the app. A default look in
/// the runtime is a house style every app would inherit, so a `framework-crates`
/// member that renders is a finding.
#[test]
fn the_framework_may_not_carry_ui() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", "pub struct C;\nimpl C {}\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            ("host-x/src/lib.rs", "fn main() { let _ = x_core::C; }\n"),
        ],
    );
    // Mark `x-core` as a framework crate: the rule only applies to those.
    let manifest = std::fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        manifest.replace("framework-crates = []", "framework-crates = [\"x-core\"]"),
    )
    .unwrap();

    let clean = summary(tmp.path());
    assert!(
        !clean.findings.iter().any(|f| f.rule == "framework-ui"),
        "a framework crate with no UI is fine"
    );

    // Now it renders.
    std::fs::write(
        tmp.path().join("x-core/src/lib.rs"),
        "pub struct C;\nimpl C {}\npub const STYLE: &str = \"font-family: serif\";\n",
    )
    .unwrap();
    let found = summary(tmp.path());
    let hit = found
        .findings
        .iter()
        .find(|f| f.rule == "framework-ui")
        .expect("a framework crate carrying styles must be named");
    assert!(hit.detail.contains("font-family"), "{}", hit.detail);
    assert!(hit.fix.contains("*-shared"), "{}", hit.fix);
}

/// A `*-core` that implements nothing is a placeholder: it compiles for wasm and
/// proves nothing about business logic, while the gate reports the wall as held.
#[test]
fn a_core_that_implements_nothing_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            (
                "x-api/src/lib.rs",
                "#[service]\npub trait XService {\n    async fn a(&self) -> u8;\n}\n",
            ),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", "pub struct Placeholder;\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            (
                "host-x/src/lib.rs",
                "fn main() {\n    let _ = x_core::Placeholder;\n}\nfn m(_s: &dyn x_api::XService) {}\n",
            ),
        ],
    );
    let found = summary(tmp.path());
    let hit = found
        .findings
        .iter()
        .find(|f| f.rule == "empty-core")
        .expect("a placeholder core must be named");
    assert!(hit.detail.contains("x-core"), "{}", hit.detail);
}
