//! CLI host: one binary, two generated clap trees, one exit-code mapping.
//!
//! Nothing here parses a method's arguments or formats a result. `#[service]`
//! generated both, from the same trait the HTTP routes came from. The host only
//! decides the binary's name, where state lives, and that a failed call exits
//! with the code the error enum maps to.

use checks_core::CheckServiceImpl;
use clap::Command;
use gabion::cli::CliExit;
use gabion::prelude::Ctx;
use incidents_core::IncidentServiceImpl;
use std::process::exit;

/// Where watches and observations live between runs.
const STATE: &str = "pulse-state.json";

fn die(message: String) -> ! {
    eprintln!("{message}");
    exit(2)
}

#[tokio::main]
async fn main() {
    let caps = match gabion_native::FileCaps::new(STATE) {
        Ok(caps) => caps,
        Err(e) => die(format!("cannot open {STATE}: {e}")),
    };
    let checks = CheckServiceImpl::new(caps.clone());
    let incidents = IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps));

    // A local CLI is its own caller: anonymous unless the generated service
    // flags (`--principal`, `--tenant`, `--trace`) name someone.
    let ctx = Ctx::anonymous();

    let matches = Command::new("pulse")
        .about("watch URLs, record outages")
        .subcommand(checks_api::__check_cli::check_cli_command())
        .subcommand(incidents_api::__incident_cli::incident_cli_command())
        .get_matches();

    let outcome = match matches.subcommand() {
        Some(("check", sub)) => {
            checks_api::__check_cli::check_cli_dispatch(&checks, &ctx, sub).await
        }
        Some(("incident", sub)) => {
            incidents_api::__incident_cli::incident_cli_dispatch(&incidents, &ctx, sub).await
        }
        _ => Err(CliExit::new(
            gabion::prelude::ErrorKind::InvalidInput,
            "choose a `check` or `incident` command",
        )),
    };

    match outcome {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("{}", e.message);
            exit(e.code);
        }
    }
}
