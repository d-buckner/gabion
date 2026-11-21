//! CLI surface support: the generated clap dispatch returns this on error so the
//! service error enum maps to a process exit code (one mapping, everywhere).
//!
//! The exit carries the same flat [`WireError`] the HTTP route sends, not only a
//! code and a message. That is what makes the four surfaces comparable: a
//! conformance suite driving one input through the CLI and the route sees the same
//! failure envelope, so "the CLI dropped the error detail" is a diff rather than a
//! difference nobody wrote down.

use crate::error::{ErrorKind, ServiceError, WireError};
use serde::Serialize;

/// A failed CLI invocation: exit code derived from `ErrorKind::exit_code`, plus
/// the wire envelope every other surface would have sent.
#[derive(Debug)]
pub struct CliExit {
    pub code: i32,
    pub message: String,
    /// The failure as the HTTP route and the MCP tool would report it.
    pub wire: WireError,
}

impl CliExit {
    /// A failure with no typed detail: a bad `--json`, an unknown subcommand.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let wire = WireError::new(kind, message);
        Self {
            code: wire.kind.exit_code(),
            message: wire.message.clone(),
            wire,
        }
    }

    /// A service failure, projected onto the wire exactly as the route does — one
    /// mapping, so the CLI cannot drift from the surface it shares an error model
    /// with.
    #[must_use]
    pub fn from_service<E: Serialize>(error: &ServiceError<E>) -> Self {
        let kind = error.kind;
        let message = error.message.clone();
        let wire = WireError::from_service(error)
            .unwrap_or_else(|_| WireError::new(kind, "error detail failed to serialize"));
        Self {
            code: kind.exit_code(),
            message,
            wire,
        }
    }
}

impl std::fmt::Display for CliExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.wire)
    }
}

/// Render a successful op result for the terminal.
///
/// A result that will not serialize must fail the call. Writing nothing and
/// exiting 0 is the worst available answer: it is indistinguishable from a
/// call that succeeded with an empty payload.
pub fn render_json<T: Serialize>(value: &T) -> Result<String, CliExit> {
    crate::serde_json::to_string_pretty(value).map_err(|e| {
        CliExit::new(
            ErrorKind::Internal,
            format!("result did not serialize: {e}"),
        )
    })
}

/// Adapt any `io::Write` to the `fmt::Write` a dispatch writes to.
///
/// A host has `io::Stdout`; the dispatch takes `fmt::Write` so a test can hand it a
/// `String` and compare what a terminal would have shown. This is the one line that
/// joins the two, and it lives here so no host writes it twice.
pub struct IoWriter<W: std::io::Write>(pub W);

impl<W: std::io::Write> core::fmt::Write for IoWriter<W> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_all(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}

/// Write one already-rendered line to the caller's writer.
///
/// A host that renders something gabion did not produce — a UI listing, say — still
/// reports a failed write as a failed run rather than exiting 0 on a closed pipe.
pub fn write_line(text: &str, out: &mut dyn core::fmt::Write) -> Result<(), CliExit> {
    writeln!(out, "{text}")
        .map_err(|e| CliExit::new(ErrorKind::Internal, format!("stdout failed: {e}")))
}

/// The same render, written straight to the caller's writer.
pub fn render_json_to<T: Serialize>(
    value: &T,
    out: &mut dyn core::fmt::Write,
) -> Result<(), CliExit> {
    let text = render_json(value)?;
    writeln!(out, "{text}")
        .map_err(|e| CliExit::new(ErrorKind::Internal, format!("stdout failed: {e}")))
}

/// Render a stream as newline-delimited JSON: one item per line, written as it
/// arrives, so a listen op that never ends still says something.
///
/// The caller supplies the writer. `gabion` is runtime-free and a library does not
/// choose a process's stdout; a host passes stdout, and the conformance suite passes
/// a `String` so the four-surface comparison sees exactly what a terminal would.
///
/// A listen op's items are plain values: a core signals failure by returning `Err`
/// from the op itself, before any item, and that failure reaches `CliExit` the same
/// way an ordinary op's does. There is no mid-stream error to project here, and
/// inventing one would be a second error channel the wire does not have.
pub async fn render_ndjson<S, T>(stream: S, out: &mut dyn core::fmt::Write) -> Result<(), CliExit>
where
    S: crate::stream::Stream<Item = T> + Unpin,
    T: Serialize,
{
    let mut stream = stream;
    while let Some(item) = crate::stream::next(&mut stream).await {
        let line = crate::serde_json::to_string(&item).map_err(|e| {
            CliExit::new(
                ErrorKind::Internal,
                format!("stream item did not serialize: {e}"),
            )
        })?;
        writeln!(out, "{line}")
            .map_err(|e| CliExit::new(ErrorKind::Internal, format!("stdout failed: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the exit code
mod tests {
    use super::*;

    #[derive(Debug, serde::Serialize)]
    enum Demo {
        NotFound,
    }

    #[test]
    fn a_service_failure_keeps_the_kind_and_the_detail() {
        let error: ServiceError<Demo> =
            ServiceError::new(ErrorKind::NotFound, "todo not found", Demo::NotFound);
        let exit = CliExit::from_service(&error);
        assert_eq!(exit.code, ErrorKind::NotFound.exit_code());
        assert_eq!(exit.wire.kind, ErrorKind::NotFound);
        assert_eq!(exit.wire.detail, serde_json::json!("NotFound"));
        assert_eq!(exit.to_string(), "not_found: todo not found");
    }

    /// The code on the struct and the kind's own code are the same number: a CLI
    /// that reported one and meant the other is a wrapper script's problem.
    #[test]
    fn the_exit_code_is_derived_from_the_kind() {
        let exit = CliExit::new(ErrorKind::InvalidInput, "bad --json");
        assert_eq!(exit.code, 2);
        assert_eq!(exit.wire.kind, ErrorKind::InvalidInput);
    }

    /// A listen op prints one line per item and nothing else, so a caller can pipe
    /// it into `jq`.
    // tokio is a dev-dependency only off wasm, and these two need an executor to
    // drive a stream; the render itself is target-independent.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn a_stream_prints_one_line_per_item() {
        let mut out = String::new();
        let items: crate::stream::BoxStream<'_, serde_json::Value> = crate::stream::from_vec(vec![
            serde_json::json!({"n": 1}),
            serde_json::json!({"n": 2}),
        ]);
        render_ndjson(items, &mut out).await.expect("clean stream");
        assert_eq!(out, "{\"n\":1}\n{\"n\":2}\n");
    }

    /// An item that will not serialize fails the run. Printing nothing and exiting 0
    /// would look like a stream that carried nothing.
    // tokio is a dev-dependency only off wasm, and these two need an executor to
    // drive a stream; the render itself is target-independent.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn a_stream_item_that_will_not_serialize_fails_the_call() {
        struct Broken;
        impl Serialize for Broken {
            fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("no"))
            }
        }
        let mut out = String::new();
        let items: crate::stream::BoxStream<'_, Broken> = crate::stream::from_vec(vec![Broken]);
        let exit = render_ndjson(items, &mut out)
            .await
            .expect_err("must not look like success");
        assert_eq!(exit.code, ErrorKind::Internal.exit_code());
        assert!(
            exit.message.contains("did not serialize"),
            "{}",
            exit.message
        );
    }

    #[test]
    fn a_value_that_will_not_serialize_fails_the_call() {
        struct Broken;
        impl Serialize for Broken {
            fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("no"))
            }
        }
        let exit = render_json(&Broken).expect_err("must not look like success");
        assert_eq!(exit.code, ErrorKind::Internal.exit_code());
        assert!(
            exit.message.contains("did not serialize"),
            "{}",
            exit.message
        );
    }
}
