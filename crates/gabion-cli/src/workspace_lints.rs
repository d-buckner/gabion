//! `[workspace.lints]` integrity: the Cargo.toml wall is audited the same way
//! `clippy_cfg` audits clippy.toml.
//!
//! Two failures are common and both are silent. The block is never added, so the
//! spec's "unsafe_code = forbid, workspace-wide" describes nothing. Or it is
//! added and no member opts into it, because `[workspace.lints]` is inert until
//! each crate says `[lints] workspace = true`. Either way the build stays green.

use std::path::Path;

use crate::config::Config;
use crate::graph::Graph;
use crate::report::{Finding, Fix};

const RULE: &str = "workspace-lints";

#[must_use]
pub fn check(root: &Path, graph: &Graph, cfg: &Config) -> Vec<Finding> {
    let path = root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![Finding::new(
            RULE,
            "Cargo.toml",
            "no workspace Cargo.toml to read the lints from",
            "the wall lives in [workspace.lints]; without it nothing is forbidden.",
        )];
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return vec![Finding::new(
            RULE,
            "Cargo.toml",
            "the workspace Cargo.toml does not parse",
            "cargo would refuse this too; fix the syntax.",
        )];
    };

    let lints = value.get("workspace").and_then(|w| w.get("lints"));
    let mut out = if lints.is_none() {
        vec![Finding::new(
            RULE,
            "Cargo.toml",
            "no [workspace.lints] section",
            "set the wall once for every member: unsafe_code = \"forbid\" and the clippy denies.",
        )]
    } else {
        let mut out = Vec::new();
        for (table, required) in [
            ("rust", &cfg.required_rust_lints),
            ("clippy", &cfg.required_clippy_lints),
        ] {
            let have = lints.and_then(|l| l.get(table));
            for (lint, want) in required {
                let found = have.and_then(|t| t.get(lint)).and_then(toml::Value::as_str);
                let Some(found) = found else {
                    out.push(Finding::new(
                        RULE,
                        format!("[workspace.lints.{table}]"),
                        format!("required lint `{lint}` is missing"),
                        "do not delete a deny to make a build go green; move the effect into an effect home instead.",
                    ));
                    continue;
                };
                if rank(found) < rank(want) {
                    out.push(Finding::new(
                        RULE,
                        format!("[workspace.lints.{table}]"),
                        format!("`{lint} = \"{found}\"` is looser than the gabion limit (\"{want}\")"),
                        "the level was lowered to let something through; that belongs in an effect home, not in the wall.",
                    ));
                }
            }
        }
        out
    };

    out.extend(opt_in_findings(graph));
    out
}

/// Strictness of a lint level. Anything unrecognised ranks lowest, so a typo in
/// the wall reads as absent rather than as satisfied.
fn rank(level: &str) -> u8 {
    match level {
        "forbid" => 3,
        "deny" => 2,
        "warn" => 1,
        _ => 0,
    }
}

/// Every member must carry `[lints] workspace = true`, or the block is inert for it.
fn opt_in_findings(graph: &Graph) -> Vec<Finding> {
    let mut findings = Vec::new();
    for krate in &graph.crates {
        let manifest = krate.dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let opted = text
            .parse::<toml::Value>()
            .ok()
            .and_then(|v| v.get("lints")?.get("workspace")?.as_bool())
            .unwrap_or(false);
        if !opted {
            findings.push(
                Finding::new(
                    RULE,
                    format!("{}/Cargo.toml", krate.rel_dir),
                    format!("crate `{}` does not opt into [workspace.lints]", krate.name),
                    "add [lints] workspace = true; the workspace wall is inert for a crate that omits it. `gabion check --fix` appends it.",
                )
                // Mechanical: one table, always the same shape, appended to a file
                // the crate owns.
                .with_fix(Fix::append(
                    manifest.clone(),
                    "\n[lints]\nworkspace = true\n",
                )),
            );
        }
    }
    findings
}
