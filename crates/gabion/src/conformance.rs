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

/// Every case's four answers, plus the cases that never ran.
#[derive(Debug, Default)]
pub struct Report {
    pub agreements: Vec<Agreement>,
    /// Cases the suite could not drive — an op no adapter implements, an input
    /// that does not parse into the declared type. A case that never ran is not
    /// four adapters agreeing; counting it as one is how a suite goes green while
    /// testing nothing.
    pub rejected: Vec<String>,
}

impl Report {
    /// Collect one case's answers, keeping the raw values so a failure can show
    /// what each surface actually said rather than only that they differed.
    pub fn record(&mut self, agreement: Agreement) {
        self.agreements.push(agreement);
    }

    /// A case that could not be driven. The reason is kept, because "the suite
    /// did not run this" and "the suite ran this and it agreed" must not look the
    /// same in a report.
    pub fn reject(&mut self, op: &str, why: impl Into<String>) {
        self.rejected.push(format!("{op}: {}", why.into()));
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures().is_empty()
    }

    /// One line per disagreement, plus one per case that never ran. Ready to be a
    /// test's assertion message.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .agreements
            .iter()
            .filter_map(Agreement::disagreement)
            .collect();
        out.extend(self.rejected.iter().cloned());
        out
    }

    /// Cases that ran, which is what the summary line counts.
    #[must_use]
    pub fn ran(&self) -> usize {
        self.agreements.len()
    }

    /// A test-friendly message: what disagreed, or how many cases agreed.
    #[must_use]
    pub fn summary(&self) -> String {
        let failures = self.failures();
        if failures.is_empty() {
            return format!("all four surfaces agreed on {} case(s)", self.ran());
        }
        format!(
            "{} problem(s) across {} case(s):\n  {}",
            failures.len(),
            self.ran() + self.rejected.len(),
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

/// What one surface delivered for a listen op: the items, and how it ended.
///
/// Termination is part of the answer. A surface that dropped the last third of a
/// stream and reported it as finished would otherwise look identical to one that
/// delivered everything, which is the fail-open §7.4 names.
#[must_use]
pub fn stream_value(items: Vec<Value>, end: &str) -> Value {
    serde_json::json!({ "items": items, "end": end })
}

/// A stream that never started, as the suite compares it. The refusal is the
/// result, so a surface that refuses is reported rather than skipped.
#[must_use]
pub fn refused_value(why: &str) -> Value {
    serde_json::json!({ "items": [], "end": why })
}

/// Drain a stream into the JSON values the suite compares.
///
/// Takes the marker shape any surface returns — anything that yields items — so
/// the in-process impl and the generated client are collected by the same code
/// and cannot be collected differently.
pub async fn collect<S>(mut stream: S) -> Vec<Value>
where
    S: crate::stream::Stream + Unpin,
    S::Item: Serialize,
{
    let mut out = Vec::new();
    while let Some(item) = crate::stream::next(&mut stream).await {
        out.push(ok_value(&item));
    }
    out
}

/// The CLI's newline-delimited output, read back as items.
///
/// A line that is not JSON is a surface that printed prose where the contract
/// promised items, so it is reported as an item that failed rather than skipped.
#[must_use]
pub fn ndjson_value(text: &str) -> Value {
    let mut items = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => items.push(v),
            Err(e) => return refused_value(&format!("stdout line is not JSON: {e}")),
        }
    }
    stream_value(items, "done")
}

/// What the MCP stdio transport answered for a listen op: the items it sent as
/// progress notifications, and the end its final result reported.
#[must_use]
pub fn mcp_stream_value(reply: &str, notifications: &[String]) -> Value {
    let Ok(answer) = serde_json::from_str::<Value>(reply) else {
        return refused_value("<the mcp reply is not JSON>");
    };
    if let Some(error) = answer.get("error") {
        return refused_value(&error.to_string());
    }
    let end = answer
        .pointer("/result/structuredContent/end")
        .and_then(Value::as_str)
        .unwrap_or("<no end reported>");
    let mut items = Vec::new();
    for line in notifications {
        let Ok(note) = serde_json::from_str::<Value>(line) else {
            return refused_value("<a progress notification is not JSON>");
        };
        match note
            .pointer("/params/message")
            .and_then(Value::as_str)
            .map(serde_json::from_str::<Value>)
        {
            Some(Ok(item)) => items.push(item),
            // A notification the suite cannot read is a surface that sent
            // something other than an item, which is a disagreement to show.
            _ => return refused_value("<a progress notification carried no item>"),
        }
    }
    stream_value(items, end)
}

/// A failure that arrived as a bare `WireError` — from the MCP tool path or from
/// the CLI's exit — as the suite compares it.
#[must_use]
pub fn wire_value(wire: &WireError) -> Value {
    serde_json::to_value(wire)
        .unwrap_or_else(|e| Value::String(format!("<wire error did not serialize: {e}>")))
}

/// A value a surface returned as text (the CLI renders pretty JSON). Text that
/// does not parse is compared as text, so a surface that started writing prose is
/// a disagreement rather than a silence.
#[must_use]
pub fn text_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
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

    /// A case that could not be driven fails the report. Otherwise a typo in the
    /// case list is a green suite with one fewer case in it.
    #[test]
    fn a_rejected_case_is_a_failure() {
        let mut report = Report::default();
        report.record(clean());
        report.reject("todo.nope", "no adapter implements this op");
        assert!(!report.is_clean());
        assert_eq!(report.ran(), 1);
        assert!(report.summary().contains("todo.nope"));
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
