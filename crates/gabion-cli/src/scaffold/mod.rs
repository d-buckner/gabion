//! `gabion new` and `gabion feature add`: scaffolding that copies the walls.
//!
//! The gap this closes (§4) was the one that most limited anyone else using the
//! framework: every app started by copying the example and hoping the copy
//! included `[workspace.lints]`, `clippy.toml`, the edge table and the surface
//! lock. A missing `clippy.toml` is not a style problem — it is the effect ban
//! (§3) being absent, and nothing fails until someone puts `std::fs` in a core.
//!
//! So the scaffold is generated from the same values the gate audits
//! (`Config::default()`), and `tests/new.rs` runs `gabion check --fast` on what it
//! produces. A scaffold that a freshly generated app fails is a scaffold that lies.

mod templates;
mod walls;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::Config;

use walls::svc_name;
pub use walls::{add_members, clippy_toml, workspace_lints};

/// Files to write, keyed by path relative to the workspace root.
pub struct Scaffold {
    files: BTreeMap<PathBuf, String>,
}

impl Scaffold {
    fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    fn add(&mut self, rel: impl Into<PathBuf>, body: String) {
        self.files.insert(rel.into(), body);
    }

    /// Every path this scaffold would write, relative to the root.
    #[must_use]
    pub fn paths(&self) -> Vec<&Path> {
        self.files.keys().map(PathBuf::as_path).collect()
    }

    /// Write the files, refusing to overwrite anything that already exists: a
    /// scaffold that clobbers a file is a scaffold nobody runs twice.
    pub fn write(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        let existing: Vec<&Path> = self
            .files
            .keys()
            .map(PathBuf::as_path)
            .filter(|rel| root.join(rel).exists())
            .collect();
        if !existing.is_empty() {
            return Err(format!(
                "refusing to overwrite {} file(s) that already exist: {}",
                existing.len(),
                existing
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let mut written = Vec::new();
        for (rel, body) in &self.files {
            let path = root.join(rel);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            std::fs::write(&path, body)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            written.push(path);
        }
        Ok(written)
    }
}

/// A whole app: workspace walls, one feature (api/core/render), two hosts.
///
/// `gabion_path` is the directory holding the gabion crates — v0.1 is
/// path-dependency only, so a scaffold has to know where the framework lives.
#[must_use]
pub fn app(name: &str, gabion_path: &Path, cfg: &Config) -> Scaffold {
    let mut scaffold = Scaffold::new();
    let feature = "todo";
    let svc = svc_name(feature);
    let gabion = gabion_path.display().to_string();
    let fill = |body: &str| {
        body.replace("%NAME%", name)
            .replace("%FEATURE%", feature)
            .replace("%SVC%", &svc)
            .replace("%GABION%", &gabion)
    };
    let members = [
        format!("features/{feature}/{feature}-api"),
        format!("features/{feature}/{feature}-core"),
        format!("features/{feature}/{feature}-render"),
        format!("hosts/{name}-host-server"),
        format!("hosts/{name}-host-native"),
    ]
    .iter()
    .map(|m| format!("    \"{m}\","))
    .collect::<Vec<_>>()
    .join("\n");

    scaffold.add(
        "Cargo.toml",
        fill(templates::ROOT_MANIFEST)
            .replace("%MEMBERS%", &members)
            // `%LINTS%` is its own template, not a placeholder in the file.
            .replace("%LINTS%", &workspace_lints(cfg)),
    );
    scaffold.add("clippy.toml", clippy_toml(cfg));
    scaffold.add(
        "rust-toolchain.toml",
        templates::hosts::TOOLCHAIN.to_string(),
    );
    scaffold.add(".gitignore", templates::hosts::GITIGNORE.to_string());
    scaffold.add("README.md", fill(templates::README));

    scaffold.add(
        format!("features/{feature}/{feature}-api/Cargo.toml"),
        fill(templates::API_MANIFEST),
    );
    scaffold.add(
        format!("features/{feature}/{feature}-api/src/lib.rs"),
        fill(templates::API_LIB),
    );
    scaffold.add(
        format!("features/{feature}/{feature}-core/Cargo.toml"),
        fill(templates::CORE_MANIFEST),
    );
    scaffold.add(
        format!("features/{feature}/{feature}-core/src/lib.rs"),
        fill(templates::CORE_LIB),
    );
    scaffold.add(
        format!("features/{feature}/{feature}-render/Cargo.toml"),
        fill(templates::RENDER_MANIFEST),
    );
    scaffold.add(
        format!("features/{feature}/{feature}-render/src/lib.rs"),
        fill(templates::RENDER_LIB),
    );
    scaffold.add(
        format!("hosts/{name}-host-server/Cargo.toml"),
        fill(templates::hosts::SERVER_MANIFEST),
    );
    scaffold.add(
        format!("hosts/{name}-host-server/src/main.rs"),
        fill(templates::hosts::SERVER_MAIN),
    );
    scaffold.add(
        format!("hosts/{name}-host-native/Cargo.toml"),
        fill(templates::hosts::NATIVE_MANIFEST),
    );
    scaffold.add(
        format!("hosts/{name}-host-native/src/main.rs"),
        fill(templates::hosts::NATIVE_MAIN),
    );

    scaffold
}

/// The three crates of one more feature, for `gabion feature add`.
///
/// The lock and the workspace members are the caller's job: the members list is
/// an edit to a file the app owns, and the lock is the app's reviewed artifact.
#[must_use]
pub fn feature(name: &str, feature: &str, cfg: &Config) -> Scaffold {
    let mut scaffold = Scaffold::new();
    let svc = svc_name(feature);
    let fill = |body: &str| {
        body.replace("%NAME%", name)
            .replace("%FEATURE%", feature)
            .replace("%SVC%", &svc)
    };
    let _ = cfg;
    for (suffix, manifest, lib) in [
        ("api", templates::API_MANIFEST, templates::API_LIB),
        ("core", templates::CORE_MANIFEST, templates::CORE_LIB),
        ("render", templates::RENDER_MANIFEST, templates::RENDER_LIB),
    ] {
        scaffold.add(
            format!("features/{feature}/{feature}-{suffix}/Cargo.toml"),
            fill(manifest),
        );
        scaffold.add(
            format!("features/{feature}/{feature}-{suffix}/src/lib.rs"),
            fill(lib),
        );
    }
    scaffold
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // a failing assertion names the value
mod tests {
    use super::*;

    #[test]
    fn the_clippy_config_carries_every_ban_the_gate_requires() {
        let cfg = Config::default();
        let text = clippy_toml(&cfg);
        for ban in &cfg.required_bans {
            assert!(text.contains(ban), "the scaffold dropped the ban on {ban}");
        }
        for (key, value) in &cfg.max_thresholds {
            assert!(
                text.contains(&format!("{key} = {value}")),
                "{key} is missing"
            );
        }
        // It is a clippy.toml, so it has to parse as one.
        let _: toml::Value = text.parse().expect("the generated clippy.toml parses");
    }

    #[test]
    fn the_lints_block_carries_every_lint_the_gate_requires() {
        let cfg = Config::default();
        let text = workspace_lints(&cfg);
        for (lint, level) in cfg
            .required_rust_lints
            .iter()
            .chain(&cfg.required_clippy_lints)
        {
            assert!(
                text.contains(&format!("{lint} = \"{level}\"")),
                "{lint} = {level} is missing"
            );
        }
    }

    #[test]
    fn a_scaffold_writes_a_parseable_workspace() {
        let cfg = Config::default();
        let app = app("demo", Path::new("/framework"), &cfg);
        let manifest = app
            .files
            .get(Path::new("Cargo.toml"))
            .expect("the scaffold writes a manifest");
        let parsed: toml::Value = manifest.parse().expect("the manifest parses");
        let members = parsed["workspace"]["members"].as_array().expect("members");
        assert_eq!(members.len(), 5);
        assert!(parsed["workspace"]["lints"]["rust"]["unsafe_code"].is_str());
        assert!(parsed["workspace"]["metadata"]["gabion"]["edges"].is_table());
        // No placeholder survived anywhere.
        for (path, body) in &app.files {
            assert!(
                !body.contains('%'),
                "{} still has a placeholder",
                path.display()
            );
        }
    }

    #[test]
    fn members_are_added_without_losing_the_rest_of_the_manifest() {
        let manifest = "[workspace]\nresolver = \"2\"\nmembers = [\n    \"a\",\n]\n\n# keep me\n[workspace.package]\nversion = \"0.1.0\"\n";
        let out = add_members(manifest, &["b".to_string()]).expect("members added");
        let parsed: toml::Value = out.parse().expect("still parses");
        assert_eq!(parsed["workspace"]["members"].as_array().unwrap().len(), 2);
        assert!(out.contains("# keep me"), "the edit ate a comment");
    }

    #[test]
    fn a_manifest_without_members_is_an_error_not_an_empty_edit() {
        let err = add_members("[workspace]\nresolver = \"2\"\n", &["b".into()]).unwrap_err();
        assert!(err.contains("no `members"), "{err}");
    }
}
