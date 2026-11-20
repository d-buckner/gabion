//! Fixture workspaces shared by the rule tests.
//!
//! Each `tests/*.rs` file is its own crate, so shared code lives in a module both
//! include rather than in one file that everything else reaches into.
#![allow(dead_code)] // each test file uses a subset

use std::collections::BTreeSet;
use std::path::Path;

use gabion_cli::config::Config;
use gabion_cli::{check, Options};

/// Write a workspace from a path -> contents map.
pub fn write_ws(dir: &Path, files: &[(&str, &str)]) {
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

pub fn rules_found(root: &Path) -> BTreeSet<&'static str> {
    let mut opts = Options::new(root);
    opts.fast = true;
    let summary = check(&opts);
    summary.findings.iter().map(|f| f.rule).collect()
}

pub fn member(name: &str, deps: &[&str]) -> String {
    member_with(name, deps, "")
}

/// A member whose `[dependencies]` also carries non-workspace entries, which is
/// how the `core-runtime-dep` fixture names a runtime.
pub fn member_with(name: &str, deps: &[&str], external: &str) -> String {
    let deps: String = deps
        .iter()
        .map(|d| format!("{d} = {{ path = \"../{d}\" }}\n"))
        .collect();
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}{external}\n[lints]\nworkspace = true\n"
    )
}

/// The `[workspace.lints]` block gabion requires, written from the same config
/// the rule reads. A fixture that copies the required list by hand would pass
/// even if the required list went empty.
pub fn honest_workspace_lints() -> String {
    let cfg = Config::default();
    let mut out = String::from("\n[workspace.lints.rust]\n");
    for (lint, level) in &cfg.required_rust_lints {
        out.push_str(&format!("{lint} = \"{level}\"\n"));
    }
    out.push_str("\n[workspace.lints.clippy]\n");
    for (lint, level) in &cfg.required_clippy_lints {
        out.push_str(&format!("{lint} = \"{level}\"\n"));
    }
    out
}

pub fn ws_head(members: &[&str]) -> String {
    ws_head_with_lints(members, &honest_workspace_lints())
}

pub fn ws_head_with_lints(members: &[&str], lints: &str) -> String {
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
{lints}"
    )
}

/// A clippy.toml carrying exactly the bans and thresholds gabion requires.
pub fn honest_clippy_toml() -> String {
    let cfg = Config::default();
    let mut out = String::from("disallowed-types = [\n");
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

/// Contract: two public ops. Both reach every surface — that is the design,
/// and the surface lock is what makes that visible in review.
pub const X_API_SRC: &str = "\
#[service]
pub trait XService {
    async fn a(&self) -> u8;
    async fn b(&self) -> u8;
}
";

/// Write the surface lock the way an app would: `--update-surfaces`, once the
/// sources exist. A fixture that writes lock lines by hand would keep passing
/// after the line format changed, which is exactly the drift the lock exists to
/// catch.
pub fn generate_lock(root: &Path) {
    let mut opts = Options::new(root);
    opts.fast = true;
    opts.update_surfaces = true;
    let summary = check(&opts);
    // `no-wasm-proof` is a fact about the fixture, not about the lock: these
    // workspaces are one or two crates with no *-core. Everything else would mean
    // the lock could not be written.
    let failed: Vec<&str> = summary
        .findings
        .iter()
        .filter(|f| !matches!(f.rule, "surface-snapshot" | "no-wasm-proof"))
        .map(|f| f.rule)
        .collect();
    assert!(
        failed.is_empty(),
        "generating the lock should be the only thing that changes: {failed:?}"
    );
}
