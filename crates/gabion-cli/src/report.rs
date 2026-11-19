//! Report: findings with named rules, and the one exit code gabion promises.
//!
//! The spec's rule: "the error message is the documentation". Every finding
//! carries the rule id, what violated it, and the fix — so an agent with zero
//! context can satisfy the gate without reading the spec.

use std::fmt;

/// A single architecture or style violation.
pub struct Finding {
    /// Stable rule id, e.g. `process-wall`.
    pub rule: &'static str,
    /// Where: `crate` plus optional `file:line`.
    pub location: String,
    /// What violated it.
    pub detail: String,
    /// How to fix it correctly (not how to suppress it).
    pub fix: &'static str,
}

impl Finding {
    #[must_use]
    pub fn new(
        rule: &'static str,
        location: impl Into<String>,
        detail: impl Into<String>,
        fix: &'static str,
    ) -> Self {
        Self {
            rule,
            location: location.into(),
            detail: detail.into(),
            fix,
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "FAIL {}", self.rule)?;
        writeln!(f, "  at:     {}", self.location)?;
        writeln!(f, "  what:   {}", self.detail)?;
        write!(f, "  fix:    {}", self.fix)
    }
}

/// Summary line printed at the end; `ok` is what drives the exit code.
pub struct Summary {
    pub findings: Vec<Finding>,
    /// Names of the steps that ran (e.g. `clippy`, `wasm`).
    pub steps: Vec<String>,
    /// Steps that failed (a failing subprocess is reported separately).
    pub failed_steps: Vec<String>,
}

impl Summary {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.findings.is_empty() && self.failed_steps.is_empty()
    }

    pub fn print(&self) {
        for finding in &self.findings {
            println!("{finding}\n");
        }
        for step in &self.failed_steps {
            println!("FAIL step:{step}\n");
        }
        if self.ok() {
            println!(
                "gabion check: PASS ({} steps: {})",
                self.steps.len(),
                self.steps.join(", ")
            );
        } else {
            println!(
                "gabion check: {} rule violation(s), {} failing step(s)",
                self.findings.len(),
                self.failed_steps.len()
            );
        }
    }
}
