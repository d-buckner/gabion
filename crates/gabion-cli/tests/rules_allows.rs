//! Who may hold an `#[allow(clippy::unwrap_used)]`, and who may not.
//!
//! The grant is a region, not a lint level: a test module, a generated module, or
//! a declared effect home. Every one of these fixtures pairs a granted allow with
//! an ungranted one, because a grant that swallows both proves nothing.
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

/// A test module gated on more than `test` alone is still a test module. tokio
/// does not build for wasm32, so an executor-backed test is written
/// `#[cfg(all(test, not(target_arch = "wasm32")))]`, and losing the grant there
/// would push that allow into shipped source.
#[test]
fn a_test_module_gated_on_more_than_test_alone_keeps_its_grant() {
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
                "pub fn f() {}\n\
                 #[cfg(all(test, not(target_arch = \"wasm32\")))]\n\
                 #[allow(clippy::unwrap_used)]\n\
                 mod tests {\n    #[test]\n    fn t() { let _ = Some(1u8).unwrap(); }\n}\n\
                 #[allow(clippy::unwrap_used)]\n\
                 mod shipped {\n    pub fn g() -> u8 { Some(1u8).unwrap() }\n}\n",
            ),
            ("host-x/Cargo.toml", &member("host-x", &["x-core"])),
            ("host-x/src/lib.rs", "fn main() { let _ = x_core::f(); }\n"),
        ],
    );
    let found = summary(tmp.path());
    let hits: Vec<&str> = found
        .findings
        .iter()
        .filter(|f| f.rule == "allow-allowlist")
        .map(|f| f.location.as_str())
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "only the allow outside a test module is a finding: {hits:?}"
    );
    assert!(
        hits[0].ends_with(":8)"),
        "the finding should be the shipped module's allow, line 8: {}",
        hits[0]
    );
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
