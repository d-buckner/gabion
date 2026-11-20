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
    // One entry per line, and the closing bracket keeps its own line: a line-based
    // edit (and a human reading the diff) both depend on that.
    let mut insertion: String = members.iter().map(|m| format!("    \"{m}\",\n")).collect();
    if !body[..close].ends_with('\n') {
        insertion.insert(0, '\n');
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

/// Remove paths from the workspace `members` array, textually, and verify the
/// result still parses.
///
/// A member that is not listed is not an error: `gabion feature remove` is also how
/// someone cleans up a half-added feature, and refusing that would mean hand-editing
/// the manifest anyway.
pub fn remove_members(manifest: &str, members: &[String]) -> Result<String, String> {
    let wanted: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    let mut out: Vec<String> = Vec::new();
    for line in manifest.lines() {
        let closes = line.trim_end().ends_with(']');
        let trimmed = line.trim().trim_end_matches(']').trim_end_matches(',');
        if wanted.iter().any(|w| w == trimmed) {
            // Dropping the entry must not drop the bracket it shared a line with.
            if closes {
                out.push("]".to_string());
            }
            continue;
        }
        // A removed entry must not leave a gap behind it.
        if line.trim().is_empty()
            && out
                .last()
                .is_some_and(|prev: &String| prev.trim().is_empty())
        {
            continue;
        }
        out.push(line.to_string());
    }
    let body = out.join("\n") + "\n";
    body.parse::<toml::Value>()
        .map_err(|e| format!("removing members left the manifest unparseable: {e}"))?;
    Ok(body)
}
