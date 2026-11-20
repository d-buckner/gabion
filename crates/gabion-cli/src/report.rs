//! Report: findings with named rules, and the one exit code gabion promises.
//!
//! The spec's rule: "the error message is the documentation". Every finding
//! carries the rule id, what violated it, and the fix — so an agent with zero
//! context can satisfy the gate without reading the spec.

use std::fmt;
use std::path::PathBuf;

/// A change `gabion check --fix` can make without judgement: the rule already
/// decided what is wrong, and the repair is the same text edit every time.
///
/// Only mechanical findings get one. A finding whose fix needs a decision (move
/// this type to `*-api`, split this function) carries advice and nothing else,
/// because a tool that rewrites architecture is a tool nobody can review.
#[derive(Debug, Clone)]
pub enum Fix {
    /// Replace one line's text. `from` is checked first, so a stale line number is
    /// refused rather than rewriting the wrong line.
    Replace {
        path: PathBuf,
        /// 1-based.
        line: usize,
        from: String,
        to: String,
    },
    /// Add text at the end of the file — how a missing table or key is repaired
    /// without rewriting a file the app owns.
    Append { path: PathBuf, text: String },
}

impl Fix {
    #[must_use]
    pub fn replace(
        path: impl Into<PathBuf>,
        line: usize,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Self {
        Self::Replace {
            path: path.into(),
            line,
            from: from.into(),
            to: to.into(),
        }
    }

    #[must_use]
    pub fn append(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self::Append {
            path: path.into(),
            text: text.into(),
        }
    }

    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        match self {
            Fix::Replace { path, .. } | Fix::Append { path, .. } => path,
        }
    }
}

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
    /// The mechanical repair, when there is one.
    pub mechanical: Option<Fix>,
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
            mechanical: None,
        }
    }

    /// Attach the text edit `--fix` should make.
    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.mechanical = Some(fix);
        self
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
    /// Plain lines to print before the findings: what a scaffolding command did,
    /// and what to run next.
    pub notes: Vec<String>,
    pub findings: Vec<Finding>,
    /// Names of the steps that ran (e.g. `clippy`, `wasm`).
    pub steps: Vec<String>,
    /// Steps that failed (a failing subprocess is reported separately).
    pub failed_steps: Vec<String>,
    /// Steps skipped *on purpose*, with the reason. A skipped gate is named in
    /// the same output as a passing one: "not installed" and "ran and passed"
    /// must never look alike.
    pub skipped: Vec<String>,
}

impl Summary {
    /// A summary with nothing to report. `--help` and `--version` write their own
    /// text and run no check, so the verdict line would be a lie — and worse,
    /// it would land on stdout and corrupt `gabion --version > file`.
    #[must_use]
    pub fn silent() -> Self {
        Self {
            notes: Vec::new(),
            findings: Vec::new(),
            steps: Vec::new(),
            failed_steps: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Nothing ran and nothing was found, so there is no verdict to print.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.notes.is_empty()
            && self.findings.is_empty()
            && self.steps.is_empty()
            && self.failed_steps.is_empty()
            && self.skipped.is_empty()
    }

    #[must_use]
    pub fn ok(&self) -> bool {
        self.findings.is_empty() && self.failed_steps.is_empty()
    }

    pub fn print(&self) {
        if self.is_silent() {
            return;
        }
        for note in &self.notes {
            println!("{note}");
        }
        for finding in &self.findings {
            println!("{finding}\n");
        }
        for step in &self.failed_steps {
            println!("FAIL step:{step}\n");
        }
        for step in &self.skipped {
            println!("SKIP step:{step}\n");
        }
        if self.ok() {
            let skipped = if self.skipped.is_empty() {
                String::new()
            } else {
                format!(" ({} skipped)", self.skipped.len())
            };
            println!(
                "gabion check: PASS ({} steps: {}){skipped}",
                self.steps.len(),
                self.steps.join(", ")
            );
        } else {
            println!(
                "gabion check: {} rule violation(s), {} failing step(s), {} skipped",
                self.findings.len(),
                self.failed_steps.len(),
                self.skipped.len()
            );
        }
    }
}
