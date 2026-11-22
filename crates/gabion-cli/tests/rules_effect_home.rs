//! The effect-home class. An app's own effects — a pty, a serial port, a C
//! library — need a place the edge table knows about, or the app folds them into
//! a host and no second host can share them.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{member, member_with, rules_found, write_ws, ws_head, X_API_SRC};
use gabion_cli::config::{Class, Config};

#[test]
fn a_native_crate_is_an_effect_home_not_a_stray() {
    let cfg = Config::default();
    assert_eq!(
        cfg.classify("dh-native", "runtimes/dh-native"),
        Class::EffectHome,
        "`*-native` already meant effect home to `is_effect_home`; classify must agree"
    );
    assert!(cfg.is_effect_home("dh-native", Class::EffectHome));
}

#[test]
fn a_listed_crate_is_an_effect_home_whatever_it_is_called() {
    let cfg = Config {
        effect_homes: vec!["dh-effects".to_string()],
        ..Config::default()
    };
    assert_eq!(
        cfg.classify("dh-effects", "runtimes/dh-effects"),
        Class::EffectHome,
        "the fix text of `unclassified-crate` promises this list"
    );
}

#[test]
fn a_host_written_in_native_code_stays_a_host() {
    let cfg = Config::default();
    assert_eq!(
        cfg.classify("host-native", "hosts/host-native"),
        Class::Host
    );
}

#[test]
fn a_host_wires_an_effect_home() {
    let cfg = Config::default();
    assert!(
        cfg.may_depend_on(Class::Host, Class::EffectHome),
        "a composition root that cannot construct its own effects re-implements them"
    );
}

#[test]
fn an_effect_home_reaches_the_runtime_and_the_contracts_only() {
    let cfg = Config::default();
    for to in [Class::Framework, Class::Shared, Class::Api] {
        assert!(
            cfg.may_depend_on(Class::EffectHome, to),
            "effect-home -> {} should be allowed",
            to.key()
        );
    }
    for to in [Class::Core, Class::Render, Class::Host, Class::EffectHome] {
        assert!(
            !cfg.may_depend_on(Class::EffectHome, to),
            "effect-home -> {} should be refused",
            to.key()
        );
    }
}

const HOST_SRC: &str = "pub fn wire() {}\n";
const NATIVE_SRC: &str = "pub fn open() {}\n";
const CORE_SRC: &str = "pub fn f() -> u8 { 1 }\n";

/// A workspace whose effect home is a crate of its own, wired by the host.
#[test]
fn a_shared_effect_home_clears_the_edge_table() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "dh-native", "host-x"])),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
            (
                "dh-native/Cargo.toml",
                &member_with("dh-native", &["x-api"], "tokio = \"1\"\n"),
            ),
            ("dh-native/src/lib.rs", NATIVE_SRC),
            (
                "host-x/Cargo.toml",
                &member("host-x", &["x-api", "dh-native"]),
            ),
            ("host-x/src/lib.rs", HOST_SRC),
        ],
    );
    let found = rules_found(tmp.path());
    for rule in ["unclassified-crate", "edge-table", "effect-home-dependency"] {
        assert!(
            !found.contains(rule),
            "{rule} fired where an effect home fits: {found:?}"
        );
    }
}

/// The same crate named `*-core` instead would be refused twice over: a core may
/// not name a runtime, and a core may not be an effect home.
#[test]
fn a_core_that_reaches_for_the_effect_home_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &ws_head(&["x-api", "x-core", "dh-native"])),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", X_API_SRC),
            (
                "x-core/Cargo.toml",
                &member("x-core", &["x-api", "dh-native"]),
            ),
            ("x-core/src/lib.rs", CORE_SRC),
            ("dh-native/Cargo.toml", &member("dh-native", &["x-api"])),
            ("dh-native/src/lib.rs", NATIVE_SRC),
        ],
    );
    let found = rules_found(tmp.path());
    assert!(
        found.contains("effect-home-dependency"),
        "a core may not depend on an effect home, got {found:?}"
    );
}
