//! The commands behind `gabion new`, `gabion feature add` and `gabion dev`.
//!
//! Split from `cli` so the arg parser stays a parser: the branching here is about
//! what a command does to a filesystem, not about reading a flag.

use std::path::PathBuf;

use crate::cli::{FeatureOptions, NewOptions};
use crate::report::Summary;

/// `gabion new`: write a whole app.
pub fn new_app(opts: &NewOptions) -> Summary {
    let dir = target_dir(opts);
    if let Some(refusal) = refuse_occupied(&dir) {
        return refusal;
    }
    let gabion = match &opts.gabion_path {
        Some(path) => path.clone(),
        None => default_gabion_path(),
    };
    if !gabion.join("crates/gabion/Cargo.toml").exists() {
        return failure(format!(
            "{} does not look like the gabion source tree (no crates/gabion/Cargo.toml). \
             Pass --gabion-path PATH.",
            gabion.display()
        ));
    }
    let cfg = crate::config::Config::default();
    let scaffold = crate::scaffold::app(&opts.name, &gabion, &cfg);
    match scaffold.write(&dir) {
        Ok(written) => Summary {
            skipped: Vec::new(),
            notes: vec![
                format!("created {} file(s) in {}", written.len(), dir.display()),
                match refresh_lock(&dir) {
                    Ok(ops) => format!(
                        "the scaffold copied the walls: clippy.toml, [workspace.lints], the edge \
                         table, and a surface lock with {ops} reviewed operation(s)"
                    ),
                    Err(e) => format!(
                        "the walls are written but the surface lock could not be generated \
                         ({e}); run `gabion check --update-surfaces` once the workspace builds"
                    ),
                },
                format!(
                    "next: cd {} && cargo build -p {}-host-server",
                    dir.display(),
                    opts.name
                ),
                "then: cargo run -p gabion-cli -- check".to_string(),
            ],
            findings: Vec::new(),
            steps: Vec::new(),
            failed_steps: Vec::new(),
        },
        Err(e) => failure(format!("could not write the scaffold: {e}")),
    }
}

fn target_dir(opts: &NewOptions) -> PathBuf {
    if opts.dir.as_os_str() == "." {
        return PathBuf::from(&opts.name);
    }
    opts.dir.clone()
}

/// A directory with files in it is somebody's work: refuse rather than merge.
fn refuse_occupied(dir: &std::path::Path) -> Option<Summary> {
    if !dir.exists() {
        return None;
    }
    let empty = std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none());
    if empty {
        return None;
    }
    Some(failure(format!(
        "{} already exists and is not empty",
        dir.display()
    )))
}

/// The gabion source tree this binary was built from. v0.1 is path-dependency
/// only, so a scaffold has to point at a checkout; `--gabion-path` overrides it.
fn default_gabion_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// `gabion feature add`: three crates, three workspace members, and the lock
/// entries for the operations the generated trait publishes.
pub fn add_feature(opts: &FeatureOptions) -> Summary {
    let manifest_path = opts.root.join("Cargo.toml");
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(e) => return failure(format!("cannot read {}: {e}", manifest_path.display())),
    };
    let name = match workspace_name(&manifest) {
        Ok(name) => name,
        Err(e) => return failure(e),
    };
    let feature_dir = opts.root.join("features").join(&opts.name);
    if feature_dir.exists() {
        return failure(format!(
            "feature `{}` already exists at {}",
            opts.name,
            feature_dir.display()
        ));
    }
    let scaffold = crate::scaffold::feature(&name, &opts.name, &crate::config::Config::default());
    let members: Vec<String> = ["api", "core", "render"]
        .iter()
        .map(|suffix| format!("features/{}/{}-{suffix}", opts.name, opts.name))
        .collect();
    let updated = match crate::scaffold::add_members(&manifest, &members) {
        Ok(text) => text,
        Err(e) => return failure(e),
    };
    if let Err(e) = scaffold.write(&opts.root) {
        return failure(e);
    }
    if let Err(e) = std::fs::write(&manifest_path, updated) {
        return failure(format!("cannot write {}: {e}", manifest_path.display()));
    }
    let operations = refresh_lock(&opts.root).unwrap_or(0);
    Summary {
        skipped: Vec::new(),
        notes: vec![
            format!(
                "added feature `{}`: {}-api | {}-core | {}-render",
                opts.name, opts.name, opts.name, opts.name
            ),
            format!(
                "workspace members updated; .gabion/surfaces.lock now lists {operations} \
                 operation(s) — review them like an API change"
            ),
            "next: fill in the impl, then `gabion check`".to_string(),
        ],
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: Vec::new(),
    }
}

/// Write the surface lock with the gate's own collector, and report how many
/// operations it found.
///
/// The scaffold does not format lock lines itself: a line carries a hash of the
/// operation's signature, so a hand-written one would be stale the first time a
/// type changed. Running the collector makes a scaffolded app's lock correct by
/// construction.
fn refresh_lock(root: &std::path::Path) -> Result<usize, String> {
    let cfg = crate::config::Config::load(root)?;
    let graph = crate::graph::load(root, &cfg)?;
    if let Some(finding) = crate::surfaces::check(&graph, root, true)
        .into_iter()
        .find(|f| f.rule == "surface-snapshot")
    {
        return Err(finding.detail);
    }
    let text = std::fs::read_to_string(root.join(crate::surfaces::LOCK_PATH))
        .map_err(|e| format!("cannot read the lock back: {e}"))?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .count())
}

/// The app name, which `gabion new` writes into `[workspace.metadata.gabion]`.
///
/// It cannot be derived from a member path: `features/todo/todo-api` carries the
/// *feature*, and the app name is only in the crate name, which is itself
/// derived. So it is declared once, where `gabion check` already reads config.
fn workspace_name(manifest: &str) -> Result<String, String> {
    let parsed: toml::Value = manifest
        .parse()
        .map_err(|e| format!("the workspace Cargo.toml does not parse: {e}"))?;
    parsed
        .get("workspace")
        .and_then(|w| w.get("metadata"))
        .and_then(|m| m.get("gabion"))
        .and_then(|g| g.get("app-name"))
        .and_then(toml::Value::as_str)
        .map(String::from)
        .ok_or_else(|| {
            "the workspace Cargo.toml has no `[workspace.metadata.gabion] app-name`; \
             add one (the crate prefix, e.g. `app-name = \"demo\"`)"
                .to_string()
        })
}

/// `gabion dev`: once, or in a loop.
pub fn dev(opts: &crate::dev::Options) -> Summary {
    if opts.once {
        return match crate::dev::once(opts) {
            Ok(0) => pass("ran once, exit 0"),
            Ok(code) => failure(format!("the process exited {code}")),
            Err(e) => failure(e),
        };
    }
    match crate::dev::watch(opts) {
        Ok(()) => pass("dev loop ended"),
        Err(e) => failure(e),
    }
}

fn pass(note: &str) -> Summary {
    Summary {
        skipped: Vec::new(),
        notes: vec![note.to_string()],
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: Vec::new(),
    }
}

fn failure(message: String) -> Summary {
    Summary {
        skipped: Vec::new(),
        notes: Vec::new(),
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: vec![message],
    }
}

/// `gabion feature remove`: the crates, the members, and the published operations.
///
/// Deleting a feature is the one scaffolding command that can leave a workspace
/// that no longer builds, so it refuses when something still depends on the feature
/// — a host that imports `billing_api` would fail `cargo metadata` after the
/// directory is gone, and reporting that as "the lock could not be refreshed" would
/// hide the real cause.
pub fn remove_feature(opts: &FeatureOptions) -> Summary {
    let manifest_path = opts.root.join("Cargo.toml");
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(e) => return failure(format!("cannot read {}: {e}", manifest_path.display())),
    };
    let feature_dir = opts.root.join("features").join(&opts.name);
    if !feature_dir.exists() {
        return failure(format!(
            "no feature `{}` at {}",
            opts.name,
            feature_dir.display()
        ));
    }
    if let Some(user) = still_depended_on(&opts.root, &opts.name) {
        return failure(format!(
            "`{user}` still depends on the `{}` feature; remove that dependency first",
            opts.name
        ));
    }
    let members: Vec<String> = ["api", "core", "render"]
        .iter()
        .map(|suffix| format!("features/{}/{}-{suffix}", opts.name, opts.name))
        .collect();
    let updated = match crate::scaffold::remove_members(&manifest, &members) {
        Ok(text) => text,
        Err(e) => return failure(e),
    };
    if let Err(e) = std::fs::remove_dir_all(&feature_dir) {
        return failure(format!("cannot remove {}: {e}", feature_dir.display()));
    }
    if let Err(e) = std::fs::write(&manifest_path, updated) {
        return failure(format!("cannot write {}: {e}", manifest_path.display()));
    }
    let operations = refresh_lock(&opts.root).unwrap_or(0);
    Summary {
        skipped: Vec::new(),
        notes: vec![
            format!(
                "removed feature `{}`: {}-api | {}-core | {}-render",
                opts.name, opts.name, opts.name, opts.name
            ),
            format!(
                "workspace members updated; .gabion/surfaces.lock now lists {operations} \
                 operation(s) — the operations that were published are gone"
            ),
            "next: `gabion check`".to_string(),
        ],
        findings: Vec::new(),
        steps: Vec::new(),
        failed_steps: Vec::new(),
    }
}

/// The first manifest that still names the feature's crates, other than the
/// feature's own.
fn still_depended_on(root: &std::path::Path, feature: &str) -> Option<String> {
    let needle = format!("-{feature}-");
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if matches!(name, "target" | ".git" | "dist") {
            continue;
        }
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            // The feature's own crates live under `features/<feature>/`, and they
            // name each other; anything outside that directory is a real dependent.
            let own = dir
                .to_string_lossy()
                .contains(&format!("features/{feature}/"));
            if !own && text.contains(&needle) {
                return Some(dir.display().to_string());
            }
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
    None
}
