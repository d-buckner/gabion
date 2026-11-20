//! `gabion check` — one command, one exit code, green means the architecture is
//! correct.
//!
//! The split is deliberate: existing linters do what linters can do (clippy for
//! effects and complexity, rustfmt, cargo-shear, cargo-deny), `cargo metadata`
//! supplies the crate graph, and gabion adds only what nothing else can see —
//! the edge table, the reachable surface, and the audits that stop an app from
//! quietly deleting a ban.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

pub mod cli;
pub mod clippy_cfg;
pub mod commands;
pub mod config;
pub mod dev;
pub mod edges;
pub mod graph;
pub mod lint;
pub mod report;
pub mod scaffold;
pub mod source;
pub mod steps;
pub mod surfaces;
pub mod workspace_lints;

use std::path::PathBuf;

use config::Config;
use report::{Finding, Summary};

pub struct Options {
    /// Workspace root (the directory holding the workspace Cargo.toml).
    pub root: PathBuf,
    /// Skip the subprocess steps (used by the checker's own tests).
    pub fast: bool,
    /// Skip the external tools (cargo-shear, cargo-deny) too.
    pub no_external: bool,
    /// wasm target for the per-core wall proof.
    pub wasm_target: String,
    /// Rewrite `.gabion/surfaces.lock` instead of checking against it.
    pub update_surfaces: bool,
    /// Apply the mechanical findings (a `pub` that should be `pub(crate)`, the
    /// surface lock) and re-run. Findings that need a decision are left alone.
    pub fix: bool,
    /// Report a missing external tool as a skip instead of a failure. Explicit,
    /// because "the licence audit did not run" and "the licence audit passed" must
    /// never be the same output.
    pub allow_missing_tools: bool,
}

impl Options {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fast: false,
            no_external: false,
            wasm_target: "wasm32-unknown-unknown".to_string(),
            update_surfaces: false,
            fix: false,
            allow_missing_tools: false,
        }
    }
}

/// Run every static rule, then the subprocess gates.
#[must_use]
pub fn check(opts: &Options) -> Summary {
    let steps = vec![
        "config".to_string(),
        "graph".to_string(),
        "edges".to_string(),
        "clippy.toml".to_string(),
        "workspace lints".to_string(),
        "surfaces".to_string(),
        "sources".to_string(),
    ];
    let (cfg, graph) = match load(opts) {
        Ok(pair) => pair,
        Err(failed) => {
            return Summary {
                skipped: Vec::new(),
                notes: Vec::new(),
                findings: Vec::new(),
                steps,
                failed_steps: vec![failed],
            }
        }
    };

    let findings = static_findings(&graph, &cfg, &opts.root, opts.update_surfaces);
    if !opts.fix {
        return finish(opts, &graph, steps, findings, Vec::new());
    }
    // The mechanical half: the text edits the rules themselves specified, then the
    // surface lock. One pass, so `--fix` cannot loop, and what is left afterwards
    // is what a human has to decide.
    let applied = apply_fixes(&findings);
    let lock = surfaces::check(&graph, &opts.root, true);
    let remaining = static_findings(&graph, &cfg, &opts.root, false);
    let notes = vec![format!(
        "--fix: {} text edit(s) applied and the surface lock rewritten; {} finding(s) need a \
         decision",
        applied + usize::from(lock.is_empty()),
        remaining.len()
    )];
    finish(opts, &graph, steps, remaining, notes)
}

/// Everything that needs no subprocess.
fn static_findings(
    graph: &graph::Graph,
    cfg: &Config,
    root: &std::path::Path,
    update: bool,
) -> Vec<Finding> {
    let mut findings = edges::check(graph, cfg);
    findings.extend(clippy_cfg::check(root, cfg));
    findings.extend(workspace_lints::check(root, graph, cfg));
    findings.extend(surfaces::check(graph, root, update));
    findings.extend(lint::visibility::check(graph));
    if !graph.crates.iter().any(|c| c.class == config::Class::Core) {
        findings.push(Finding::new(
            "no-wasm-proof",
            "workspace",
            "no *-core crate, so the wasm wall proof has nothing to build",
            "every feature ships a *-core that must compile for wasm32; without one the wall \
             is not being checked, and a vacuous pass is worse than a red one. Run \
             `gabion feature add <name>`.",
        ));
    }
    for krate in &graph.crates {
        findings.extend(lint::check_crate(krate, cfg));
    }
    findings
}

/// Load config and graph, or say which one failed.
fn load(opts: &Options) -> Result<(Config, graph::Graph), String> {
    let cfg = Config::load(&opts.root).map_err(|e| format!("config: {e}"))?;
    let graph = graph::load(&opts.root, &cfg).map_err(|e| format!("graph: {e}"))?;
    if graph.crates.is_empty() {
        return Err(
            "graph: `cargo metadata` reported no workspace members, so every rule below had \
             nothing to check"
                .to_string(),
        );
    }
    Ok((cfg, graph))
}

/// Run the subprocess gates and assemble the summary.
fn finish(
    opts: &Options,
    graph: &graph::Graph,
    mut steps: Vec<String>,
    findings: Vec<Finding>,
    notes: Vec<String>,
) -> Summary {
    let mut failed_steps = Vec::new();
    let mut skipped = Vec::new();
    if !opts.fast {
        let mut to_run = steps::Step::core();
        if !opts.no_external {
            to_run.extend(steps::Step::external_tools());
        }
        to_run.extend(steps::Step::wasm_builds(graph, &opts.wasm_target));
        for step in to_run {
            steps.push(step.name.to_string());
            match steps::run(&step, &opts.root) {
                Ok(()) => {}
                Err(steps::StepFailure::Missing { tool, install }) => {
                    if opts.allow_missing_tools {
                        skipped.push(format!("{tool}: not installed ({install})"));
                    } else {
                        failed_steps.push(format!("{tool}: not installed ({install})"));
                    }
                }
                Err(steps::StepFailure::Failed(why)) => failed_steps.push(why),
            }
        }
    }
    Summary {
        skipped,
        notes,
        findings,
        steps,
        failed_steps,
    }
}

/// Apply every mechanical fix. Returns how many were applied.
///
/// A file is edited by replacing the exact text on the reported line, and the edit
/// is refused when that text is no longer there — a stale line number must not
/// rewrite the wrong `pub`.
fn apply_fixes(findings: &[Finding]) -> usize {
    let mut by_file: std::collections::BTreeMap<&std::path::Path, Vec<&report::Fix>> =
        std::collections::BTreeMap::new();
    for fix in findings.iter().filter_map(|f| f.mechanical.as_ref()) {
        by_file.entry(fix.path.as_path()).or_default().push(fix);
    }
    let mut applied = 0;
    for (path, fixes) in by_file {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let mut changed = false;
        for fix in fixes {
            let Some(line) = lines.get_mut(fix.line.saturating_sub(1)) else {
                continue;
            };
            if !line.contains(&fix.from) {
                continue;
            }
            *line = line.replacen(&fix.from, &fix.to, 1);
            applied += 1;
            changed = true;
        }
        if changed {
            let mut body = lines.join("\n");
            body.push('\n');
            let _ = std::fs::write(path, body);
        }
    }
    applied
}
