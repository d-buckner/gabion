//! The crate graph, taken from `cargo metadata` rather than parsed by hand.
//!
//! Cargo already resolves workspace members, glob member patterns, and renamed
//! path dependencies. Re-deriving that from Cargo.toml files would be a second,
//! worse cargo — so we ask cargo and read JSON.

use std::path::{Path, PathBuf};

use crate::config::{Class, Config};

/// One workspace member, classified.
pub struct Crate {
    pub name: String,
    /// Directory holding its Cargo.toml.
    pub dir: PathBuf,
    /// That directory relative to the workspace root, for messages.
    pub rel_dir: String,
    pub class: Class,
    pub is_effect_home: bool,
    /// Workspace-internal dependencies (path deps), by crate name.
    pub ws_deps: Vec<String>,
    /// Whether this crate produces an executable — what `gabion dev` can run.
    pub has_bin: bool,
}

impl Crate {
    /// `file:line` suffix for a source file of this crate.
    #[must_use]
    pub fn at(&self, file: &Path, line: usize) -> String {
        let rel = file.strip_prefix(&self.dir).unwrap_or(file);
        format!(
            "{}:{} ({}:{})",
            self.name,
            rel.display(),
            self.rel_dir,
            line
        )
    }

    #[must_use]
    pub fn source_files(&self) -> Vec<PathBuf> {
        let src = self.dir.join("src");
        let mut out = Vec::new();
        collect_rs(&src, &mut out);
        out.sort();
        out
    }
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

pub struct Graph {
    pub root: PathBuf,
    pub crates: Vec<Crate>,
}

impl Graph {
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Crate> {
        self.crates.iter().find(|c| c.name == name)
    }
}

/// Ask cargo for the workspace graph and classify every member.
pub fn load(root: &Path, cfg: &Config) -> Result<Graph, String> {
    let out = std::process::Command::new("cargo")
        .arg("metadata")
        .args(["--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("cannot run `cargo metadata`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cannot parse `cargo metadata` output: {e}"))?;
    let root_dir = json
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "`cargo metadata` output has no workspace_root".to_string())?;

    let mut crates = Vec::new();
    let Some(packages) = json.get("packages").and_then(serde_json::Value::as_array) else {
        return Ok(Graph {
            root: root_dir,
            crates,
        });
    };
    for pkg in packages {
        let Some(c) = read_package(pkg, &root_dir, cfg) else {
            continue;
        };
        crates.push(c);
    }
    crates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Graph {
        root: root_dir,
        crates,
    })
}

fn read_package(pkg: &serde_json::Value, root: &Path, cfg: &Config) -> Option<Crate> {
    let name = pkg.get("name")?.as_str()?.to_string();
    let manifest = PathBuf::from(pkg.get("manifest_path")?.as_str()?);
    let dir = manifest.parent()?.to_path_buf();
    let rel_dir = dir
        .strip_prefix(root)
        .unwrap_or(&dir)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");

    let class = cfg.classify(&name, &rel_dir);
    let is_effect_home = cfg.is_effect_home(&name, class);
    let ws_deps = workspace_deps(pkg);
    Some(Crate {
        name,
        dir,
        rel_dir,
        class,
        is_effect_home,
        ws_deps,
        has_bin: produces_bin(pkg),
    })
}

/// A crate with a `bin` (or a `cdylib` with a `main`) is something `gabion dev`
/// can run; a library is not.
fn produces_bin(pkg: &serde_json::Value) -> bool {
    pkg.get("targets")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|targets| {
            targets.iter().any(|t| {
                t.get("kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|kinds| {
                        kinds
                            .iter()
                            .any(|k| k.as_str() == Some("bin") || k.as_str() == Some("example"))
                    })
            })
        })
}

/// Path dependencies (no registry source) — the edges the wall is made of.
fn workspace_deps(pkg: &serde_json::Value) -> Vec<String> {
    let Some(deps) = pkg
        .get("dependencies")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = deps
        .iter()
        .filter(|d| d.get("source").is_none_or(|v| v.is_null()))
        .filter_map(|d| d.get("name").and_then(serde_json::Value::as_str))
        .map(String::from)
        .collect();
    out.sort();
    out.dedup();
    out
}
