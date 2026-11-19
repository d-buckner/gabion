//! CLI surface support: the generated clap dispatch returns this on error so
//! the service error enum maps to a process exit code (one mapping, everywhere).

use crate::error::ErrorKind;
use serde::Serialize;

/// A failed CLI invocation: exit code derived from `ErrorKind::exit_code`.
#[derive(Debug)]
pub struct CliExit {
    pub code: i32,
    pub message: String,
}

/// Render a successful op result for the terminal.
///
/// A result that will not serialize must fail the call. Writing nothing and
/// exiting 0 is the worst available answer: it is indistinguishable from a
/// call that succeeded with an empty payload.
pub fn render_json<T: Serialize>(value: &T) -> Result<String, CliExit> {
    crate::serde_json::to_string_pretty(value).map_err(|e| CliExit {
        code: ErrorKind::Internal.exit_code(),
        message: format!("result did not serialize: {e}"),
    })
}
