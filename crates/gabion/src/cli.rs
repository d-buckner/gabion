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
