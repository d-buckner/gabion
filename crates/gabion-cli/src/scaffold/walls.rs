//! Generating the walls: the config files a scaffold copies, and the two text
//! edits it makes to files the app owns.
//!
//! These are generated from `Config::default()` — the values `gabion check`
//! audits — rather than pasted, so a scaffolded app cannot disagree with its own
//! gate. A pasted copy of `clippy.toml` is a copy that will drift.

use crate::config::Config;

/// The `clippy.toml` an app needs, built from the same `Config` the gate audits.
/// A pasted copy could drift; this cannot, because `clippy-config` reads the same
/// defaults.
#[must_use]
pub fn clippy_toml(cfg: &Config) -> String {
    let mut out = String::from(
        "# The effect ban and the size gates `gabion check` audits.\n\
         #\n\
         # These are not style preferences. `std::time`, `std::env` and\n\
         # `std::process` compile for wasm32 and trap at runtime, so the target\n\
         # does not catch them; this list does. The space and line limits are the\n\
         # shortcut they seal: one 200-line match instead of a named function.\n\n",
    );
    for (key, value) in &cfg.max_thresholds {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out.push_str("\ndisallowed-types = [\n");
    for path in &cfg.required_bans {
        out.push_str(&format!(
            "    {{ path = \"{path}\", reason = \"core is runtime-free: use a Caps capability\" }},\n"
        ));
    }
    out.push_str("]\n");
    out
}

/// `[workspace.lints]`, from the same defaults the gate audits.
#[must_use]
pub fn workspace_lints(cfg: &Config) -> String {
    let mut out = String::from("# The rustc and clippy walls. Every member opts in with\n# `[lints] workspace = true`; a member that does not is a `workspace-lints`\n# finding, because a wall nobody opted into is a wall that is not there.\n[workspace.lints.rust]\n");
    for (lint, level) in &cfg.required_rust_lints {
        out.push_str(&format!("{lint} = \"{level}\"\n"));
    }
    out.push_str("\n[workspace.lints.clippy]\n");
    for (lint, level) in &cfg.required_clippy_lints {
        out.push_str(&format!("{lint} = \"{level}\"\n"));
    }
    for lint in [
        "cognitive_complexity",
        "too_many_lines",
        "too_many_arguments",
        "type_complexity",
    ] {
        out.push_str(&format!("{lint} = \"deny\"\n"));
    }
    out
}

/// Merge new operation lines into a surface lock, keeping the header comment and
/// the sort order `--update-surfaces` writes.
///
/// `gabion feature add` does this rather than telling the user to run the gate:
/// the lock's purpose is to make publishing an op a reviewed diff, and a diff the
/// command writes in the same commit is exactly that. Leaving it stale would make
/// the first thing a new feature does is fail the gate.
#[must_use]
pub fn add_lock_entries(lock: &str, entries: &[String]) -> String {
    let mut header: Vec<&str> = lock
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .collect();
    if header.is_empty() {
        header.push("# gabion public surface. Every line is reachable over CLI, HTTP,");
        header.push("# MCP and the typed client. Review changes here like an API change.");
    }
    let mut ops: Vec<String> = lock
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .chain(entries.iter().cloned())
        .collect();
    ops.sort();
    ops.dedup();
    let mut out = header.join("\n");
    out.push('\n');
    for op in ops {
        out.push_str(&op);
        out.push('\n');
    }
    out
}

/// The operations a scaffolded feature publishes, in the lock's line format.
#[must_use]
pub fn lock_entries(name: &str, feature: &str) -> Vec<String> {
    let svc = svc_name(feature);
    ["create", "list"]
        .iter()
        .map(|op| format!("{name}-{feature}-api::{svc}Service::{op}"))
        .collect()
}

/// Add paths to the workspace `members` array, textually, so comments and layout
/// survive. Verified by re-parsing: a members list that no longer parses is a
/// workspace that no longer builds, and the caller gets told.
pub fn add_members(manifest: &str, members: &[String]) -> Result<String, String> {
    let open = manifest
        .find("members = [")
        .ok_or("the workspace Cargo.toml has no `members = [` list")?;
    let close = manifest[open..]
        .find(']')
        .map(|i| open + i)
        .ok_or("the `members` list is not closed")?;
    let mut body = manifest.to_string();
    let mut insertion = String::new();
    for member in members {
        insertion.push_str(&format!("\n    \"{member}\","));
    }
    body.insert_str(close, &insertion);
    let parsed: toml::Value = body
        .parse()
        .map_err(|e| format!("adding members left the manifest unparseable: {e}"))?;
    let listed = parsed
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
        .map_or(0, Vec::len);
    if listed < members.len() {
        return Err(
            "the members list did not grow; is it a single string instead of an array?".into(),
        );
    }
    Ok(body)
}

/// `capitalised` form of a feature folder name, for type prefixes: `todo` ->
/// `Todo`, which is what a service trait and its DTOs are named after.
pub(crate) fn svc_name(feature: &str) -> String {
    let mut chars = feature.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => feature.to_string(),
    }
}
