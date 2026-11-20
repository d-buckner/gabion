//! The subprocess half of the gate: gabion check runs the linters that already
//! exist rather than reimplementing them. One command, one exit code.

use std::path::Path;
use std::process::Command;

use crate::config::Class;
use crate::graph::Graph;

/// A gate that is an existing tool, invoked with the app's own config.
pub struct Step {
    /// Short name used in the summary and in `FAIL step:<name>`.
    pub name: &'static str,
    /// Program and args.
    pub program: &'static str,
    pub args: Vec<String>,
    /// Install hint shown when the tool is missing.
    pub install: Option<&'static str>,
}

fn step(name: &'static str, program: &'static str, args: &[&str]) -> Step {
    Step {
        name,
        program,
        args: args.iter().map(|a| (*a).to_string()).collect(),
        install: None,
    }
}

impl Step {
    /// Steps that need no toolchain beyond rustup's defaults.
    #[must_use]
    pub fn core() -> Vec<Step> {
        vec![
            step("fmt", "cargo", &["fmt", "--check"]),
            step(
                "clippy",
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            step("test", "cargo", &["test", "--workspace", "--all-features"]),
        ]
    }

    /// Extra tools CI installs; `--fast` skips them locally.
    #[must_use]
    pub fn external_tools() -> Vec<Step> {
        vec![
            Step {
                name: "shear",
                program: "cargo",
                args: vec!["shear".to_string()],
                install: Some("cargo install cargo-shear"),
            },
            Step {
                name: "deny",
                program: "cargo",
                args: vec!["deny".to_string(), "check".to_string()],
                install: Some("cargo install cargo-deny"),
            },
        ]
    }

    /// One wasm build per *-core crate: the wall proof, per feature.
    #[must_use]
    pub fn wasm_builds(graph: &Graph, target: &str) -> Vec<Step> {
        graph
            .crates
            .iter()
            .filter(|c| c.class == Class::Core)
            .map(|c| Step {
                name: "wasm",
                program: "cargo",
                args: vec![
                    "build".to_string(),
                    "-p".to_string(),
                    c.name.clone(),
                    "--target".to_string(),
                    target.to_string(),
                ],
                install: None,
            })
            .collect()
    }
}

/// Why a step did not pass.
///
/// A tool that is not installed and a tool that ran and failed are different
/// facts, and reporting the first as the second is how a missing licence audit
/// looks like a broken build (§8). The caller decides what a missing tool means —
/// `--allow-missing-tools` turns it into a named skip, and silence is not an
/// option in either case.
pub enum StepFailure {
    /// The program could not be started at all.
    Missing { tool: String, install: &'static str },
    /// It ran and did not succeed.
    Failed(String),
}

/// Run one step; `Ok(())` means green. Output is inherited so failures show
/// the tool's own diagnostics.
pub fn run(step: &Step, root: &Path) -> Result<(), StepFailure> {
    let status = Command::new(step.program)
        .args(&step.args)
        .current_dir(root)
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(StepFailure::Failed(format!(
            "`{} {}` exited {}",
            step.program,
            step.args.join(" "),
            s
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StepFailure::Missing {
            tool: format!("{} {}", step.program, step.args.join(" ")),
            install: step
                .install
                .unwrap_or("install it, or pass --allow-missing-tools"),
        }),
        Err(e) => Err(StepFailure::Failed(format!(
            "cannot run `{}`: {e}",
            step.program
        ))),
    }
}
