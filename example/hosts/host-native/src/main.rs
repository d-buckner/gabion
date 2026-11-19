//! Native host: wires NativeCaps into the service impl, hands the trait object
//! to the generated CLI, the terminal renderer, and the Dioxus desktop window.
//! Zero business logic here.
//!
//! Hosts are effect homes: reading argv/env and exiting the process is exactly
//! what a host is for. `gabion check` audits that this allow stays in hosts.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::sync::Arc;
use todo_api::TodoService;
use todo_core::TodoServiceImpl;

fn main() {
    let caps = gabion_native::FileCaps::new(
        std::env::var("MYAPP_STORE").unwrap_or_else(|_| "myapp.store.json".into()),
    )
    .unwrap_or_else(|e| {
        eprintln!("cannot open store: {e}");
        std::process::exit(1);
    });
    let svc = Arc::new(TodoServiceImpl::new(caps));
    let dynsvc: Arc<dyn TodoService + Send + Sync> = svc.clone();

    let mut cmd = clap::Command::new("myapp")
        .subcommand(todo_api::__todo_cli::todo_cli_command())
        .subcommand(clap::Command::new("ui").about("render the todo list (demo of render crate)"))
        .subcommand(clap::Command::new("gui").about("open the Dioxus desktop window"));
    let matches = cmd.clone().get_matches();

    // GUI path: Dioxus owns its async runtime, so launch OUTSIDE any tokio rt.
    if matches.subcommand_matches("gui").is_some() {
        dioxus::LaunchBuilder::new()
            .with_context(todo_render::Svc(dynsvc))
            .with_cfg(
                dioxus::desktop::Config::new()
                    .with_window(dioxus::desktop::WindowBuilder::new().with_title("myapp")),
            )
            .launch(todo_render::TodoApp);
        return;
    }

    // CLI paths: build the tokio runtime by hand (no #[tokio::main]).
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cannot start runtime: {e}");
            std::process::exit(1);
        }
    };

    let result: Result<String, gabion::cli::CliExit> = rt.block_on(async {
        match matches.subcommand() {
            Some(("ui", _)) => todo_render::render_list(dynsvc.as_ref())
                .await
                .map_err(|e| gabion::cli::CliExit {
                    code: e.kind_code.exit_code(),
                    message: e.message,
                }),
            Some(("todo", m)) => todo_api::__todo_cli::todo_cli_dispatch(dynsvc.as_ref(), m).await,
            _ => {
                cmd.print_help().ok();
                std::process::exit(0);
            }
        }
    });

    match result {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("error: {}", e.message);
            std::process::exit(e.code);
        }
    }
}
