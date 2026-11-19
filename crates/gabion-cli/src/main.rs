//! `gabion` — the framework's own command line. Effect home: a dev tool spawns
//! processes and reads the environment by definition, so the ban is lifted here
//! and `gabion check` audits that it stays lifted only in declared homes.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (status, code) = gabion_cli::cli::run(&args);
    status.print();
    code
}
