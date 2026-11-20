//! The four-adapter agreement suite (spec §8).
//!
//! Generation is not verification. Four adapters that are each hand-exercised
//! can each be green while disagreeing: the CLI lowercases ids, the route drops
//! a field, the client maps a conflict to a 500. This module is the comparison —
//! the generated suite drives one input through every surface and hands the four
//! answers here, where they are compared as JSON.
//!
//! It lives in the runtime because the comparison is the same for every app; the
//! *cases* live in the app's test, because only the app knows what a call means.

use serde::Serialize;
use serde_json::Value;

use crate::error::{ServiceError, WireError};

/// One input, driven through every surface.
#[derive(Debug, Clone)]
pub struct Case {
    /// The dotted op name, as MCP names it: `todo.create`.
    pub op: &'static str,
    /// The call's input, already JSON: the suite parses it into the method's
    /// declared type, which is the same parse every adapter performs.
    pub input: Value,
}

impl Case {
    #[must_use]
    pub fn new(op: &'static str, input: Value) -> Self {
        Self { op, input }
    }
}

/// What each of the four surfaces answered for one case.
///
/// A success is the return value; a failure is the flat `WireError`, so the two
/// are compared in one space — an op that succeeds in-proc and fails over HTTP
/// is exactly the disagreement this suite exists to catch.
#[derive(Debug, Clone)]
pub struct Agreement {
    pub op: String,
    pub inproc: Value,
    pub cli: Value,
    pub mcp: Value,
    pub http: Value,
}

impl Agreement {
    #[must_use]
    pub fn new(op: &str, inproc: Value, cli: Value, mcp: Value, http: Value) -> Self {
        Self {
            op: op.to_string(),
            inproc,
            cli,
            mcp,
            http,
        }
    }

    fn pairs(&self) -> [(&'static str, &Value); 4] {
        [
            ("in-proc", &self.inproc),
            ("cli", &self.cli),
            ("mcp", &self.mcp),
            ("http", &self.http),
        ]
    }

    /// The surfaces that disagree with the in-process call, named. `None` means
    /// every adapter agreed, which is the only pass.
    #[must_use]
    pub fn disagreement(&self) -> Option<String> {
        let differing: Vec<String> = self
            .pairs()
            .iter()
            .filter(|(name, value)| *name != "in-proc" && **value != self.inproc)
            .map(|(name, value)| format!("{name} said {}", compact(value)))
            .collect();
        if differing.is_empty() {
            return None;
        }
        Some(format!(
            "{}: in-proc said {}, but {}",
            self.op,
            compact(&self.inproc),
            differing.join("; ")
        ))
    }
}

/// Every case's four answers.
#[derive(Debug, Default)]
pub struct Report {
    pub agreements: Vec<Agreement>,
}

impl Report {
    /// Collect one case's answers, keeping the raw values so a failure can show
    /// what each surface actually said rather than only that they differed.
    pub fn record(&mut self, agreement: Agreement) {
        self.agreements.push(agreement);
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures().is_empty()
    }

    /// One line per disagreement, ready to be a test's assertion message.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        self.agreements
            .iter()
            .filter_map(Agreement::disagreement)
            .collect()
    }

    /// A test-friendly message: what disagreed, or how many cases agreed.
    #[must_use]
    pub fn summary(&self) -> String {
        let failures = self.failures();
        if failures.is_empty() {
            return format!(
                "all four surfaces agreed on {} case(s)",
                self.agreements.len()
            );
        }
        format!(
            "{} of {} case(s) disagreed across surfaces:\n  {}",
            failures.len(),
            self.agreements.len(),
            failures.join("\n  ")
        )
    }
}

/// A success value as the suite compares it. A value that will not serialize is
/// reported as such rather than as `null`: `null` would make an unserializable
/// answer look like an agreed empty one.
#[must_use]
pub fn ok_value<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(v) => v,
        Err(e) => Value::String(format!("<did not serialize: {e}>")),
    }
}

/// A failure as the suite compares it: the same flat `WireError` the route and
/// the client use, so a client-side reinterpretation of the kind is a diff.
#[must_use]
pub fn err_value<E: Serialize>(error: &ServiceError<E>) -> Value {
    match WireError::from_service(error) {
        Ok(wire) => serde_json::to_value(wire)
            .unwrap_or_else(|e| Value::String(format!("<wire error did not serialize: {e}>"))),
        Err(e) => Value::String(format!("<error detail did not serialize: {e}>")),
    }
}

/// Compact JSON for a failure message. Pretty JSON for an input the caller
/// wrote inline reads as noise inside an assertion.
fn compact(value: &Value) -> String {
    value.to_string()
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should say which surface disagreed
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use serde_json::json;

    fn clean() -> Agreement {
        Agreement::new(
            "demo.echo",
            json!({"a": 1}),
            json!({"a": 1}),
            json!({"a": 1}),
            json!({"a": 1}),
        )
    }

    #[test]
    fn four_matching_answers_agree() {
        assert!(clean().disagreement().is_none());
        let mut report = Report::default();
        report.record(clean());
        assert!(report.is_clean());
        assert_eq!(report.summary(), "all four surfaces agreed on 1 case(s)");
    }

    /// The failure that matters: one adapter answers something else. The message
    /// has to name the surface, or the test tells you only that something is
    /// wrong somewhere in four generated adapters.
    #[test]
    fn one_disagreeing_surface_is_named() {
        let broken = Agreement::new(
            "demo.echo",
            json!({"a": 1}),
            json!({"a": 1}),
            json!({"a": "1"}),
            json!({"a": 1}),
        );
        let failure = broken.disagreement().expect("mismatch");
        assert!(failure.contains("mcp said"), "got: {failure}");
        assert!(!failure.contains("cli said"), "got: {failure}");
    }

    #[test]
    fn a_success_against_a_wire_error_is_a_disagreement() {
        let error: ServiceError<&str> = ServiceError::new(ErrorKind::Conflict, "nope", "detail");
        let broken = Agreement::new(
            "demo.echo",
            json!({"a": 1}),
            err_value(&error),
            json!({"a": 1}),
            json!({"a": 1}),
        );
        assert!(broken.disagreement().is_some());
    }

    #[test]
    fn an_unserializable_value_is_not_silently_null() {
        struct Broken;
        impl Serialize for Broken {
            fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("no"))
            }
        }
        let value = ok_value(&Broken);
        assert!(value
            .as_str()
            .is_some_and(|s| s.contains("did not serialize")));
    }
}
