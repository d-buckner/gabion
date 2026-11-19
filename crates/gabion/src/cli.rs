//! CLI surface support: the generated clap dispatch returns this on error so
//! the service error enum maps to a process exit code (one mapping, everywhere).

/// A failed CLI invocation: exit code derived from `ErrorKind::exit_code`.
pub struct CliExit {
    pub code: i32,
    pub message: String,
}
