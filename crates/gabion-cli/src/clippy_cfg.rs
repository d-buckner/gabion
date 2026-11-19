//! clippy.toml integrity: the ban list is itself a wall, so it gets audited.
//!
//! A framework whose enforcement lives in one config file has a problem: the
//! same person who can add `#[allow]` can delete a line from clippy.toml, and
//! CI goes quiet. This module makes deleting a ban a named failure instead of
//! an absence of evidence.

use std::path::Path;

use crate::config::Config;
use crate::report::Finding;

#[must_use]
pub fn check(root: &Path, cfg: &Config) -> Vec<Finding> {
    let path = root.join("clippy.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![Finding::new(
            "clippy-config",
            "clippy.toml",
            "no clippy.toml at the workspace root",
            "the effect ban needs it: std::time, std::env and std::process all compile on wasm and trap only at runtime.",
        )];
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return vec![Finding::new(
            "clippy-config",
            "clippy.toml",
            "clippy.toml does not parse",
            "fix the syntax; clippy silently ignores what it cannot read.",
        )];
    };

    let banned = banned_paths(&value);
    let mut out: Vec<Finding> = cfg
        .required_bans
        .iter()
        .filter(|want| !banned.iter().any(|have| have == *want))
        .map(|want| {
            Finding::new(
                "clippy-config",
                "clippy.toml",
                format!("required effect ban `{want}` is missing"),
                "do not delete a ban to make a build go green; move the effect into an effect home instead.",
            )
        })
        .collect();

    for (key, max) in &cfg.max_thresholds {
        match value.get(key).and_then(toml::Value::as_integer) {
            None => out.push(Finding::new(
                "clippy-config",
                "clippy.toml",
                format!("required threshold `{key}` is missing"),
                "the complexity gates are part of the wall; keep the key at or below the gabion default.",
            )),
            Some(have) if have > *max => out.push(Finding::new(
                "clippy-config",
                "clippy.toml",
                format!("`{key} = {have}` is looser than the gabion limit ({max})"),
                "the threshold was loosened to let something through; split that function instead.",
            )),
            Some(_) => {}
        }
    }
    out
}

/// Every `path = "..."` entry under disallowed-types / disallowed-methods.
fn banned_paths(value: &toml::Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["disallowed-types", "disallowed-methods"] {
        let Some(entries) = value.get(key).and_then(toml::Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(path) = entry.get("path").and_then(toml::Value::as_str) {
                out.push(path.to_string());
            }
        }
    }
    out
}
