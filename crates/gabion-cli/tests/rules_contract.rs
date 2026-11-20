//! Contract shape: what the surface lock hashes, and whether anything serves a
//! contract. These are about the *set* of published operations rather than about
//! where code lives, so they live apart from the placement fixtures.
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

/// A contract no host names is a surface nobody can reach. `gabion check` is the
/// only thing that can see it: at runtime an absent mount is an absent call.
#[test]
fn a_contract_no_host_serves_is_a_finding() {
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
            ("x-core/src/lib.rs", "pub fn f() {}\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            // The host never names `XService`: nothing mounts it.
            ("host-x/src/lib.rs", "fn main() { let _ = x_core::f(); }\n"),
        ],
    );
    let found = summary(tmp.path());
    let hit = found
        .findings
        .iter()
        .find(|f| f.rule == "unserved-contract")
        .expect("an unserved contract must be named");
    assert!(hit.location.contains("XService"), "{}", hit.location);
    assert!(hit.fix.contains("mount_on"), "{}", hit.fix);
}

/// The lock's hash resolves definitions across the workspace, so a change to a type
/// in *another* crate drifts the operation that names it. The per-crate version
/// hashed it by name only, which is the case a contract shared between features
/// hits immediately.
#[test]
fn a_shared_type_changing_in_another_crate_drifts_the_lock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "other-api", "x-core", "host-x"])),
            ("clippy.toml", &honest_clippy_toml()),
            ("other-api/Cargo.toml", &member("other-api", &[])),
            (
                "other-api/src/lib.rs",
                "pub struct Shared {\n    pub a: u8,\n}\n",
            ),
            ("x-api/Cargo.toml", &member("x-api", &["other-api"])),
            (
                "x-api/src/lib.rs",
                "#[service]\npub trait XService {\n    async fn a(&self, input: other_api::Shared) -> u8;\n}\n",
            ),
            ("x-core/Cargo.toml", &member("x-core", &["x-api"])),
            ("x-core/src/lib.rs", "pub struct Impl;\nimpl Impl {}\n"),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            ("host-x/src/lib.rs", "fn main() { let _ = x_core::Impl; }\nfn m(_s: &dyn x_api::XService) {}\n"),
        ],
    );
    generate_lock(tmp.path());

    // The same trait, the same method name: only the type it names changed.
    write_ws(
        tmp.path(),
        &[(
            "other-api/src/lib.rs",
            "pub struct Shared {\n    pub a: u16,\n}\n",
        )],
    );
    let found = summary(tmp.path());
    let drift = found
        .findings
        .iter()
        .find(|f| f.rule == "surface-snapshot")
        .expect("a changed type in another crate must drift the lock");
    assert!(
        drift.detail.contains("x-api::XService::a"),
        "{}",
        drift.detail
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
