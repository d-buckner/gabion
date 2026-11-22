//! `gabion new` and `gabion feature add`, tested by running the gate on what they
//! produce.
//!
//! The gap these close was "every app starts by copying the example and hoping
//! the copy included the walls". A scaffold test that only listed files would
//! reproduce that hope, so these scaffold a real workspace into a temp directory
//! and run the real `gabion check --fast` over it — cargo metadata, the edge
//! table, the clippy-config audit, the workspace lints, the surface lock, and the
//! source rules, all against files the scaffold wrote.
// gabion-cli is a declared effect home: this test writes workspaces to a temp dir
// and reads them back, which is exactly what the tool under test does.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use gabion_cli::{cli, Options};

/// The gabion source tree this test binary was built from.
fn framework() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

fn new_app(dir: &Path, name: &str) {
    let args = vec![
        "new".to_string(),
        name.to_string(),
        "--dir".to_string(),
        dir.display().to_string(),
        "--gabion-path".to_string(),
        framework().display().to_string(),
    ];
    let (summary, code) = cli::run(&args);
    assert!(
        summary.ok(),
        "scaffolding failed: {:?}",
        summary.failed_steps
    );
    assert_eq!(code, std::process::ExitCode::SUCCESS);
}

fn check(root: &Path) -> gabion_cli::report::Summary {
    let mut opts = Options::new(root);
    opts.fast = true;
    gabion_cli::check(&opts)
}

fn findings(summary: &gabion_cli::report::Summary) -> String {
    summary
        .findings
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The whole point: what `gabion new` writes passes `gabion check`.
#[test]
fn a_scaffolded_app_passes_the_gate() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("demo");
    new_app(&root, "demo");

    let summary = check(&root);
    assert!(
        summary.ok(),
        "a freshly scaffolded app fails its own gate:\n{}\n{:?}",
        findings(&summary),
        summary.failed_steps
    );
}

/// The walls are copied, not remembered: the effect ban, the lints block, the
/// edge table, and a lock whose lines are the scaffold's own operations.
#[test]
fn the_scaffold_copies_every_wall() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("demo");
    new_app(&root, "demo");

    let clippy = std::fs::read_to_string(root.join("clippy.toml")).expect("clippy.toml");
    assert!(clippy.contains("std::time::SystemTime"), "{clippy}");
    assert!(
        clippy.contains("cognitive-complexity-threshold = 15"),
        "{clippy}"
    );

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    assert!(manifest.contains("unsafe_code = \"forbid\""), "{manifest}");
    assert!(
        manifest.contains("disallowed_types = \"deny\""),
        "{manifest}"
    );
    assert!(
        manifest.contains("[workspace.metadata.gabion.edges]"),
        "{manifest}"
    );

    let lock = std::fs::read_to_string(root.join(".gabion/surfaces.lock")).expect("lock");
    assert_eq!(
        lock.lines().filter(|l| !l.starts_with('#')).count(),
        2,
        "the lock lists the operations the scaffold publishes: {lock}"
    );

    // Every member opts into the workspace lints; the gate fails for one that
    // does not, and a scaffold that generated such a member would be a trap.
    for member in [
        "features/todo/todo-api",
        "features/todo/todo-core",
        "features/todo/todo-render",
        "hosts/demo-host-server",
        "hosts/demo-host-native",
    ] {
        let text = std::fs::read_to_string(root.join(member).join("Cargo.toml")).expect(member);
        assert!(text.contains("[lints]"), "{member} does not opt in");
    }
}

/// The scaffold points at a real gabion checkout; a path that is not one is an
/// error rather than a workspace that fails to build ten minutes later.
#[test]
fn a_wrong_gabion_path_is_refused_up_front() {
    let dir = tempfile::tempdir().expect("temp dir");
    let args = vec![
        "new".to_string(),
        "demo".to_string(),
        "--dir".to_string(),
        dir.path().join("demo").display().to_string(),
        "--gabion-path".to_string(),
        dir.path().display().to_string(),
    ];
    let (summary, _) = cli::run(&args);
    assert!(!summary.ok());
    assert!(
        summary
            .failed_steps
            .iter()
            .any(|s| s.contains("gabion source tree")),
        "{:?}",
        summary.failed_steps
    );
}

/// Scaffolding into a directory that already has files must refuse: a command
/// that clobbers a working tree is one nobody runs twice.
#[test]
fn an_existing_directory_is_not_overwritten() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("keep.txt"), "mine").expect("write");
    let args = vec![
        "new".to_string(),
        "demo".to_string(),
        "--dir".to_string(),
        dir.path().display().to_string(),
        "--gabion-path".to_string(),
        framework().display().to_string(),
    ];
    let (summary, _) = cli::run(&args);
    assert!(!summary.ok());
    assert!(dir.path().join("keep.txt").exists(), "the file was touched");
}

/// `gabion feature add` grows a feature into an app that already exists, and the
/// result still passes the gate: members added, walls inherited, lock unchanged
/// (the new feature publishes nothing until its trait declares an op).
#[test]
fn a_feature_can_be_added_to_an_existing_app() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("demo");
    new_app(&root, "demo");

    let before = std::fs::read_to_string(root.join(".gabion/surfaces.lock")).expect("lock");
    let args = vec![
        "feature".to_string(),
        "add".to_string(),
        "billing".to_string(),
        "--root".to_string(),
        root.display().to_string(),
    ];
    let (summary, _) = cli::run(&args);
    assert!(summary.ok(), "{:?}", summary.failed_steps);

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        manifest.contains("features/billing/billing-api"),
        "{manifest}"
    );
    for suffix in ["api", "core", "render"] {
        assert!(
            root.join(format!("features/billing/billing-{suffix}/src/lib.rs"))
                .exists(),
            "billing-{suffix} was not written"
        );
    }

    // The lock grew by the new trait's operations, in the same command: the diff
    // is the review, and a stale lock would make the gate fail before the feature
    // could be used.
    let after_lock = std::fs::read_to_string(root.join(".gabion/surfaces.lock")).expect("lock");
    assert!(after_lock.contains("demo-billing-api::BillingService::create"));
    assert!(
        after_lock.contains("demo-todo-api::TodoService::create"),
        "the operations that were already published were kept"
    );
    assert_ne!(after_lock, before);

    // The gate now says the one thing a new feature has not done yet: publish a
    // contract nobody serves. That is the rule working — an op that exists and
    // cannot be reached is a half-published surface — so the test asserts the
    // finding rather than a green run it would be wrong to promise.
    let after = check(&root);
    let rules: Vec<&str> = after.findings.iter().map(|f| f.rule).collect();
    assert_eq!(
        rules,
        vec!["unserved-contract"],
        "only `unserved-contract` should remain:\n{}",
        findings(&after)
    );

    // Adding the same feature twice is refused, not silently duplicated.
    let (again, _) = cli::run(&args);
    assert!(!again.ok(), "a second `feature add billing` must refuse");
}

/// `gabion dev --once` runs the command it was given and reports its status. The
/// watch loop's two pure parts (host choice, change detection) have their own
/// unit tests; this is the process boundary.
#[test]
fn dev_once_runs_the_command_and_reports_its_status() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().to_string_lossy().to_string();

    let (ok, _) = cli::run(&[
        "dev".to_string(),
        "--once".to_string(),
        "--run".to_string(),
        "true".to_string(),
        "--root".to_string(),
        root.clone(),
    ]);
    assert!(ok.ok(), "{:?}", ok.failed_steps);

    let (failed, code) = cli::run(&[
        "dev".to_string(),
        "--once".to_string(),
        // `false` rather than `exit 1`: `--run` is spawned directly, not through a
        // shell, so a shell builtin is not a command.
        "--run".to_string(),
        "false".to_string(),
        "--root".to_string(),
        root,
    ]);
    assert!(!failed.ok(), "a non-zero child must fail the command");
    assert_eq!(code, std::process::ExitCode::FAILURE);
    assert!(failed.failed_steps.iter().any(|s| s.contains("exited 1")));
}

/// `gabion feature remove` takes back the crates, the members, and the published
/// operations — and refuses when a host still depends on the feature, because the
/// workspace it would leave could not build.
#[test]
fn a_feature_can_be_removed_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("demo");
    new_app(&root, "demo");
    let add = vec![
        "feature".to_string(),
        "add".to_string(),
        "billing".to_string(),
        "--root".to_string(),
        root.display().to_string(),
    ];
    let (added, _) = cli::run(&add);
    assert!(added.ok(), "{:?}", added.failed_steps);

    // A host that names the feature: removal must refuse, not leave a broken
    // workspace behind.
    let host_manifest = root.join("hosts/demo-host-server/Cargo.toml");
    let original = std::fs::read_to_string(&host_manifest).unwrap();
    std::fs::write(
        &host_manifest,
        original.replace(
            "[dependencies]",
            "[dependencies]\ndemo-billing-api = { path = \"../../features/billing/billing-api\" }",
        ),
    )
    .unwrap();
    let remove: Vec<String> = vec![
        "feature".to_string(),
        "remove".to_string(),
        "billing".to_string(),
        "--root".to_string(),
        root.display().to_string(),
    ];
    let (refused, _) = cli::run(&remove);
    assert!(!refused.ok(), "a depended-on feature must not be removed");
    assert!(
        refused
            .failed_steps
            .iter()
            .any(|s| s.contains("still depends")),
        "{:?}",
        refused.failed_steps
    );
    assert!(
        root.join("features/billing").exists(),
        "the crates must remain"
    );

    // With the dependency gone, the removal succeeds and the gate is clean: the
    // operations left the lock with the crates.
    std::fs::write(&host_manifest, original).unwrap();
    let (removed, _) = cli::run(&remove);
    assert!(removed.ok(), "{:?}", removed.failed_steps);
    assert!(!root.join("features/billing").exists());

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("features/billing/"), "{manifest}");
    let lock = std::fs::read_to_string(root.join(".gabion/surfaces.lock")).unwrap();
    assert!(!lock.contains("BillingService"), "{lock}");
    let after = check(&root);
    assert!(
        after.ok(),
        "the app must be clean again:\n{}",
        findings(&after)
    );
}
