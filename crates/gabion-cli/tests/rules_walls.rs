//! The surface lock, the workspace lints, and the audits that keep the rulebook
//! honest. Split from `rules.rs` when the source-rule fixtures outgrew the file
//! length this framework enforces on everyone else.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{generate_lock, honest_clippy_toml, member, write_ws, ws_head, X_API_SRC};
use gabion_cli::{check, Options};

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
            ("x-api/Cargo.toml", &member("x-api", &[])),
            // One op reviewed and locked; `b` is added below, as an app would.
            (
                "x-api/src/lib.rs",
                "#[service]\npub trait XService {\n    async fn a(&self) -> u8;\n}\n",
            ),
        ],
    );
    generate_lock(tmp.path());
    write_ws(tmp.path(), &[("x-api/src/lib.rs", X_API_SRC)]);
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
        !drift.detail.contains("gone"),
        "unchanged ops must not be reported as removed: {}",
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
    assert!(
        !updated
            .findings
            .iter()
            .any(|f| f.rule == "surface-snapshot"),
        "updating the lock must clear the surface finding: {:?}",
        updated.findings.iter().map(|f| f.rule).collect::<Vec<_>>()
    );
    assert!(tmp.path().join(".gabion/surfaces.lock").is_file());

    opts.update_surfaces = false;
    assert!(
        !check(&opts)
            .findings
            .iter()
            .any(|f| f.rule == "surface-snapshot"),
        "the lock just written must satisfy the check"
    );
}
