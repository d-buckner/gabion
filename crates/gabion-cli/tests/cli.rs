//! The two invocations that run no check: `--help` and `--version`.
//!
//! These exist because both used to end with `gabion check: PASS (0 steps: )`.
//! A command that ran no check has no verdict to report, and the line landed on
//! stdout, so `gabion --version > version.txt` wrote a verdict instead of a
//! version. The assertions are on `is_silent` rather than on captured stdout,
//! because an integration test cannot capture the process's own stdout — which
//! is also why `version_line` is a function.

use gabion_cli::cli;

fn args(flag: &str) -> Vec<String> {
    vec![flag.to_string()]
}

#[test]
fn version_is_silent_and_succeeds() {
    for flag in ["-V", "--version", "version"] {
        let (summary, code) = cli::run(&args(flag));
        assert_eq!(code, std::process::ExitCode::SUCCESS, "{flag} failed");
        assert!(
            summary.is_silent(),
            "{flag} reported a check verdict: {:?}",
            summary.steps
        );
    }
}

#[test]
fn version_line_names_the_command_and_the_crate_version() {
    let line = cli::version_line();
    assert_eq!(
        line,
        format!("gabion {}", env!("CARGO_PKG_VERSION")),
        "the line must name `gabion`, the binary a person typed, not `gabion-cli`"
    );
}

#[test]
fn help_is_silent_and_succeeds() {
    for flag in ["-h", "--help", "help"] {
        let (summary, code) = cli::run(&args(flag));
        assert_eq!(code, std::process::ExitCode::SUCCESS, "{flag} failed");
        assert!(
            summary.is_silent(),
            "{flag} reported a check verdict: {:?}",
            summary.steps
        );
    }
}

#[test]
fn an_unknown_subcommand_is_a_failing_nonsilent_summary() {
    let (summary, code) = cli::run(&args("not-a-command"));
    assert_eq!(code, std::process::ExitCode::FAILURE);
    assert!(!summary.is_silent(), "a usage error must still be reported");
    assert!(!summary.ok(), "a usage error is a failure");
}
