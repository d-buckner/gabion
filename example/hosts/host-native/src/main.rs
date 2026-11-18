//! Native host: wires NativeCaps into the service impl, hands the trait object
//! to the generated CLI and to the render code. Zero business logic here.

use std::sync::Arc;
use todo_api::TodoService;
use todo_core::TodoServiceImpl;

#[tokio::main]
async fn main() {
    let caps = sietch_native::FileCaps::new(std::env::var("MYAPP_STORE")
        .unwrap_or_else(|_| "myapp.store.json".into()))
        .unwrap_or_else(|e| {
            eprintln!("cannot open store: {e}");
            std::process::exit(1);
        });
    let svc = Arc::new(TodoServiceImpl::new(caps));
    let dynsvc: Arc<dyn TodoService + Send + Sync> = svc.clone();

    let mut cmd = clap::Command::new("myapp")
        .subcommand(todo_api::__todo_cli::todo_cli_command())
        .subcommand(
            clap::Command::new("ui").about("render the todo list (demo of render crate)"),
        );
    let matches = cmd.clone().get_matches();

    let result: Result<String, sietch::cli::CliExit> = match matches.subcommand() {
        Some(("ui", _)) => todo_render::render_list(dynsvc.as_ref())
            .await
            .map_err(|e| sietch::cli::CliExit { code: e.kind_code.exit_code(), message: e.message }),
        Some(("todo", m)) => todo_api::__todo_cli::todo_cli_dispatch(dynsvc.as_ref(), m).await,
        _ => {
            cmd.print_help().ok();
            std::process::exit(0);
        }
    };

    match result {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("error: {}", e.message);
            std::process::exit(e.code);
        }
    }
}
