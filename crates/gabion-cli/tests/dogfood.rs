//! Integration tests are effect homes: they build fixture workspaces on disk
//! and unwrap freely. gabion's own source rules do not scan `tests/`, and
//! clippy's workspace bans are lifted here exactly as they are for a host crate.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
//!
//! Gabion must pass its own gate. This is the dogfood test: it runs the real
//! checker over this repository, so a wall that stops biting in the framework
//! fails CI here before it misleads an app.

use std::path::PathBuf;

use gabion_cli::{check, Options};

#[test]
fn gabion_passes_its_own_check() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .expect("crates/gabion-cli -> workspace root");
    assert!(
        root.join("Cargo.toml").is_file(),
        "workspace root not found at {root:?}"
    );

    let mut opts = Options::new(root);
    opts.fast = true;
    let summary = check(&opts);
    if !summary.ok() {
        let detail: Vec<String> = summary
            .findings
            .iter()
            .map(|f| format!("{} at {}: {}", f.rule, f.location, f.detail))
            .collect();
        panic!("gabion fails its own gate:\n{}", detail.join("\n"));
    }
}
