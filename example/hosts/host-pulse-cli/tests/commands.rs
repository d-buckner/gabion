//! The generated CLI, driven the way a shell drives it: argv in, stdout and an
//! exit code out. Nothing here is a template — these are the clap trees
//! `#[service]` built, run against the real impls over one store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_core::CheckServiceImpl;
use clap::Command;
use gabion::prelude::ErrorKind;
use gabion::testing::TestCaps;
use incidents_core::IncidentServiceImpl;

fn pulse() -> Command {
    Command::new("pulse")
        .subcommand(checks_api::__check_cli::check_cli_command())
        .subcommand(incidents_api::__incident_cli::incident_cli_command())
}

#[tokio::test]
async fn a_generated_check_command_prints_the_record_it_made() {
    let checks = CheckServiceImpl::new(TestCaps::new());
    let matches = pulse().get_matches_from([
        "pulse",
        "check",
        "add",
        "--json",
        r#"{"name":"api","url":"http://api.internal/health"}"#,
    ]);
    let sub = matches.subcommand().unwrap().1;

    let out = checks_api::__check_cli::check_cli_dispatch(&checks, sub)
        .await
        .unwrap();
    assert!(out.contains("\"name\": \"api\""), "{out}");
    assert!(out.contains("api.internal"), "{out}");
}

/// One error enum, one mapping: the same `ErrorKind` that picks the HTTP status
/// picks the exit code, and the host never writes that mapping itself.
#[tokio::test]
async fn an_unknown_id_exits_with_the_code_the_error_enum_maps_to() {
    let checks = CheckServiceImpl::new(TestCaps::new());
    let matches = pulse().get_matches_from(["pulse", "check", "probe", "--json", r#""nope""#]);
    let sub = matches.subcommand().unwrap().1;

    let err = checks_api::__check_cli::check_cli_dispatch(&checks, sub)
        .await
        .expect_err("a missing watch must not exit 0");
    assert_eq!(err.code, ErrorKind::NotFound.exit_code());
    assert!(err.message.contains("not found"), "{}", err.message);
}

/// The whole app through its thinnest surface: two features, one store, and the
/// incident text arrives from the feature that never sees the probe.
#[tokio::test]
async fn a_sweep_and_an_evaluate_over_the_cli() {
    let caps = TestCaps::new();
    let checks = CheckServiceImpl::new(caps.clone());
    let incidents = IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps.clone()));

    let add = pulse().get_matches_from([
        "pulse",
        "check",
        "add",
        "--json",
        r#"{"name":"cdn","url":"http://cdn.internal/health"}"#,
    ]);
    checks_api::__check_cli::check_cli_dispatch(&checks, add.subcommand().unwrap().1)
        .await
        .unwrap();

    for _ in 0..2 {
        checks_api::__check_cli::check_cli_dispatch(
            &checks,
            pulse()
                .get_matches_from(["pulse", "check", "sweep"])
                .subcommand()
                .unwrap()
                .1,
        )
        .await
        .unwrap();
        caps.time().advance(61_000);
    }

    let evaluate = pulse().get_matches_from(["pulse", "incident", "evaluate"]);
    let out = incidents_api::__incident_cli::incident_cli_dispatch(
        &incidents,
        evaluate.subcommand().unwrap().1,
    )
    .await
    .unwrap();
    assert!(out.contains("cdn"), "{out}");
    assert!(out.contains("no connection"), "{out}");
}
