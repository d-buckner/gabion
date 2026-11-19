//! The three rules no existing linter can express, plus the audits that keep
//! the *other* linters honest.
//!
//! Everything expressible as clippy config lives in clippy.toml instead of here.
//! What is left is genuinely gabion-specific: attributes clippy cannot see
//! (`#[surface]`), policy about the config files themselves (clippy.toml bans,
//! `#[allow]` placement), and the Dune comment rule.
//!
//! Shared source infrastructure (tokenizer, spans, grant regions) lives in
//! `crate::source`.

use std::path::Path;

use crate::config::Config;
use crate::graph::Crate;
use crate::report::Finding;
use crate::source::{regions_of, scan_comments, Comment};

mod branches;
mod rules;
use branches::branch_rule;
use rules::{allow_audit, comment_rule, host_logic_rule, silent_default_rule, surface_rules};

#[must_use]
pub fn check_crate(krate: &Crate, cfg: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    for path in krate.source_files() {
        match std::fs::read_to_string(&path) {
            Ok(content) => check_file(krate, cfg, &path, &content, &mut out),
            Err(e) => out.push(Finding::new(
                "unreadable-source",
                format!("{}:{}", krate.name, path.display()),
                format!("cannot read source: {e}"),
                "fix the file or remove it from the crate.",
            )),
        }
    }
    out
}

fn check_file(krate: &Crate, cfg: &Config, path: &Path, content: &str, out: &mut Vec<Finding>) {
    let lines = content.lines().count();
    if lines > cfg.max_file_lines {
        out.push(Finding::new(
            "file-length",
            krate.at(path, 1),
            format!("{lines} lines, limit is {}", cfg.max_file_lines),
            "a file this long is doing more than one job; split it along a real seam.",
        ));
    }

    let comments = scan_comments(content);
    out.extend(no_todo(krate, path, &comments));

    let file = match syn::parse_file(content) {
        Ok(f) => f,
        Err(e) => {
            out.push(Finding::new(
                "unparsed-source",
                krate.at(path, 1),
                format!("syn cannot parse this file: {e}"),
                "gabion's source rules cannot see an unparsed file; fix the syntax.",
            ));
            return;
        }
    };

    let regions = regions_of(&file);
    out.extend(surface_rules(krate, path, &file));
    out.extend(comment_rule(krate, path, &file, &comments, &regions));
    out.extend(silent_default_rule(krate, path, &file));
    out.extend(host_logic_rule(krate, path, &file));
    out.extend(allow_audit(krate, path, &file, &regions));
    out.extend(branch_rule(krate, path, &file, cfg.max_branches));
}

const TODO_MARKERS: [&str; 3] = ["TODO", "FIXME", "XXX"];

fn no_todo(krate: &Crate, path: &Path, comments: &[Comment]) -> Vec<Finding> {
    comments
        .iter()
        .filter(|c| !c.doc)
        .filter(|c| TODO_MARKERS.iter().any(|m| c.text.contains(m)))
        .map(|c| {
            Finding::new(
                "no-todo",
                krate.at(path, c.line),
                c.text.trim().to_string(),
                "a TODO is a shortcut with a deadline nobody owns: file the issue, or do it now.",
            )
        })
        .collect()
}
