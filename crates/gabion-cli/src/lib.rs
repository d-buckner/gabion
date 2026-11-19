//! `gabion check` — one command, one exit code, green means the architecture is
//! correct.
//!
//! The split is deliberate: existing linters do what linters can do (clippy for
//! effects and complexity, rustfmt, cargo-shear, cargo-deny), `cargo metadata`
//! supplies the crate graph, and gabion adds only what nothing else can see —
//! the edge table, `#[surface]` completeness, and the audits that stop an app
//! from quietly deleting a ban.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

pub mod cli;
pub mod clippy_cfg;
pub mod config;
pub mod edges;
pub mod graph;
pub mod lint;
pub mod report;
pub mod source;
pub mod steps;

use std::path::PathBuf;

use config::Config;
use report::Summary;

pub struct Options {
    /// Workspace root (the directory holding the workspace Cargo.toml).
    pub root: PathBuf,
    /// Skip the subprocess steps (used by the checker's own tests).
    pub fast: bool,
    /// Skip the external tools (cargo-shear, cargo-deny) too.
    pub no_external: bool,
    /// wasm target for the per-core wall proof.
    pub wasm_target: String,
}

impl Options {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fast: false,
            no_external: false,
            wasm_target: "wasm32-unknown-unknown".to_string(),
        }
    }
}

/// Run every static rule, then the subprocess gates.
#[must_use]
pub fn check(opts: &Options) -> Summary {
    let mut steps = vec![
        "config".to_string(),
        "graph".to_string(),
        "edges".to_string(),
        "clippy.toml".to_string(),
        "sources".to_string(),
    ];
    let cfg = match Config::load(&opts.root) {
        Ok(c) => c,
        Err(e) => {
            return Summary {
                findings: Vec::new(),
                steps,
                failed_steps: vec![format!("config: {e}")],
            };
        }
    };
    let graph = match graph::load(&opts.root, &cfg) {
        Ok(g) => g,
        Err(e) => {
            return Summary {
                findings: Vec::new(),
                steps,
                failed_steps: vec![format!("graph: {e}")],
            };
        }
    };

    let mut findings = edges::check(&graph, &cfg);
    findings.extend(clippy_cfg::check(&opts.root, &cfg));
    for krate in &graph.crates {
        findings.extend(lint::check_crate(krate, &cfg));
    }

    let mut failed_steps = Vec::new();
    if !opts.fast {
        let mut to_run = steps::Step::core();
        if !opts.no_external {
            to_run.extend(steps::Step::external_tools());
        }
        to_run.extend(steps::Step::wasm_builds(&graph, &opts.wasm_target));
        for step in to_run {
            steps.push(step.name.to_string());
            if let Err(e) = steps::run(&step, &opts.root) {
                failed_steps.push(format!("{}: {e}", step.name));
            }
        }
    }

    Summary {
        findings,
        steps,
        failed_steps,
    }
}
