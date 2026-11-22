//! The gate's own config is parsed strictly: a key gabion does not recognise is
//! a named failure, not a row that reads as applied while the default stays in
//! force. That silence is the class `docs/GAPS.md §1` names, so the parser that
//! reads the wall has to refuse it.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod fixtures;

use fixtures::{honest_clippy_toml, member, write_ws, ws_head};
use gabion_cli::report::Summary;
use gabion_cli::{check, Options};

/// Run the static half of the gate over a fixture.
fn fast_check(root: &std::path::Path) -> Summary {
    let mut opts = Options::new(root);
    opts.fast = true;
    check(&opts)
}

/// The message a config failure prints: `check` renders a failed load as a named
/// step, so a typo has to arrive here with its key in it, not as a bare exit code.
fn config_failure(summary: &Summary) -> String {
    summary
        .failed_steps
        .iter()
        .filter(|s| s.starts_with("config:"))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// A typo in the edge table used to insert a dead `unclassified` row and leave
/// the *default* row in force, so an app that believed it tightened a wall got
/// the looser one with nothing in the output saying so.
#[test]
fn a_typo_in_the_edge_table_is_a_finding_not_a_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let typo = ws_head(&["x-core", "x-shared"]).replace(
        "[workspace.metadata.gabion.edges]\n",
        "[workspace.metadata.gabion.edges]\ncorr = [\"framework\"]\n",
    );
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &typo),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-shared/Cargo.toml", &member("x-shared", &[])),
            ("x-shared/src/lib.rs", "pub struct S;\n"),
            ("x-core/Cargo.toml", &member("x-core", &["x-shared"])),
            ("x-core/src/lib.rs", "pub struct C;\n"),
        ],
    );
    let summary = fast_check(tmp.path());
    let failure = config_failure(&summary);
    assert!(
        failure.contains("corr"),
        "a typo'd class must be named in a config failure, got {failure:?}"
    );
    assert!(
        failure.contains("core") && failure.contains("effect-home"),
        "the failure must name the accepted class set, got {failure:?}"
    );
    assert!(!summary.ok(), "a typo'd class must fail the run");

    // And the row the app *did* spell is the one in force: `core` narrowed to
    // `framework` must not fall back to the default that allows `shared`.
    let narrowed = ws_head(&["x-core", "x-shared"]).replace(
        "core = [\"framework\", \"shared\", \"api\"]",
        "core = [\"framework\"]",
    );
    write_ws(tmp.path(), &[("Cargo.toml", &narrowed)]);
    let summary = fast_check(tmp.path());
    assert!(
        summary
            .findings
            .iter()
            .any(|f| f.location == "x-core -> x-shared"),
        "the app's own `core` row must be the one in force, got {:?}",
        summary
            .findings
            .iter()
            .map(|f| &f.location)
            .collect::<Vec<_>>()
    );
}

/// An unknown threshold key was accepted as a *stricter* requirement, so
/// `clippy_cfg` demanded it and `--fix` wrote the typo into clippy.toml, which
/// makes clippy hard-error (`unknown field name`) on every later run.
#[test]
fn an_unknown_threshold_key_is_a_finding_and_fix_does_not_write_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let typo = ws_head(&["x-api"]).replace(
        "max-branches = 2",
        "max-branches = 2\nmax-thresholds = { cognitive-complexity-threshhold = 15 }",
    );
    write_ws(
        tmp.path(),
        &[
            ("Cargo.toml", &typo),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let summary = fast_check(tmp.path());
    let failure = config_failure(&summary);
    assert!(
        failure.contains("cognitive-complexity-threshhold"),
        "the unknown threshold must be named, got {failure:?}"
    );
    assert!(
        failure.contains("cognitive-complexity-threshold"),
        "the failure must name the accepted threshold keys, got {failure:?}"
    );

    // `--fix` must not be the thing that carries the typo into clippy.toml.
    let before = std::fs::read_to_string(tmp.path().join("clippy.toml")).unwrap();
    let mut fix = Options::new(tmp.path());
    fix.fast = true;
    fix.fix = true;
    let applied = check(&fix);
    assert!(!applied.ok(), "a config failure is not a fixable finding");
    let after = std::fs::read_to_string(tmp.path().join("clippy.toml")).unwrap();
    assert_eq!(before, after, "--fix must not rewrite clippy.toml");
    assert!(
        !after.contains("threshhold"),
        "--fix wrote the typo into clippy.toml:\n{after}"
    );
}

/// A row of the wrong type used to be skipped by `if let Some(..)`, which is the
/// same silence as a typo: the config reads as applied and is not.
#[test]
fn a_malformed_config_row_is_a_finding_not_skipped() {
    let bad_edges = tempfile::tempdir().expect("tempdir");
    let head = ws_head(&["x-api"]).replace(
        "core = [\"framework\", \"shared\", \"api\"]",
        "core = \"framework\"",
    );
    write_ws(
        bad_edges.path(),
        &[
            ("Cargo.toml", &head),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let failure = config_failure(&fast_check(bad_edges.path()));
    assert!(
        failure.contains("edges") && failure.contains("core"),
        "a non-array edge row must be named, got {failure:?}"
    );

    let bad_threshold = tempfile::tempdir().expect("tempdir");
    let head = ws_head(&["x-api"]).replace(
        "max-branches = 2",
        "max-branches = 2\nmax-thresholds = { cognitive-complexity-threshold = \"fifteen\" }",
    );
    write_ws(
        bad_threshold.path(),
        &[
            ("Cargo.toml", &head),
            ("clippy.toml", &honest_clippy_toml()),
            (".gabion/surfaces.lock", ""),
            ("x-api/Cargo.toml", &member("x-api", &[])),
            ("x-api/src/lib.rs", "pub trait X {}\n"),
        ],
    );
    let failure = config_failure(&fast_check(bad_threshold.path()));
    assert!(
        failure.contains("cognitive-complexity-threshold"),
        "a non-integer threshold must be named, got {failure:?}"
    );
}
