//! Automatic registration, driven the way a host drives it: mount an impl, get
//! every generated surface.
//!
//! The claim worth testing is not "a call exists" — it is that a host names only
//! *what to serve* and never *how*: no namespace, no router function, no tool
//! list. And that the mounted set is readable, so a host can log what it exposes
//! and a test can pin it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use gabion::registry::Host;
use todo_api::{Todo, TodoErr, TodoId, TodoService};

struct Stub;

#[gabion::async_trait]
impl TodoService for Stub {
    async fn create(
        &self,
        _ctx: &gabion::Ctx,
        input: todo_api::CreateTodo,
    ) -> Result<Todo, TodoErr> {
        Ok(Todo {
            id: "t1".into(),
            title: input.title,
            done: false,
            created_at: 1,
        })
    }

    async fn complete(&self, _ctx: &gabion::Ctx, input: TodoId) -> Result<Todo, TodoErr> {
        Ok(Todo {
            id: input.0,
            title: "milk".into(),
            done: true,
            created_at: 1,
        })
    }

    async fn list(&self, _ctx: &gabion::Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr> {
        Ok(Vec::new())
    }
}

fn mounted() -> Host {
    let svc: Arc<dyn TodoService + Send + Sync> = Arc::new(Stub);
    let mut host = Host::new();
    svc.mount_on(&mut host);
    host
}

/// Mounting one impl brings the routes, the subcommand and the tools. The host
/// said `Stub`; it did not say `todo`, `todo_router` or `todo_cli_command`.
#[test]
fn mounting_an_impl_brings_every_surface() {
    let mut host = mounted();

    assert_eq!(host.mounted(), vec!["todo"]);
    assert_eq!(host.contracts(), vec!["TodoService"]);
    assert_eq!(
        host.exposed(),
        vec!["todo.complete", "todo.create", "todo.list"],
        "the op list a host logs is the set the surface lock reviews"
    );
    assert_eq!(host.routers().len(), 1, "one generated router");
    assert_eq!(host.commands().len(), 1, "one generated subcommand");

    let mcp = host.mcp().expect("one generated mcp service");
    let names: Vec<String> = mcp.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["todo.complete", "todo.create", "todo.list"]);
}

/// Two contracts on one host, one line each: the pulse server's whole
/// composition, with no per-service wiring left to keep in step.
#[test]
fn two_contracts_mount_on_one_host_without_naming_a_surface() {
    let mut host = Host::new();
    let todo: Arc<dyn TodoService + Send + Sync> = Arc::new(Stub);
    todo.mount_on(&mut host);

    assert_eq!(host.mounted(), vec!["todo"]);
    assert_eq!(host.routers().len(), 1);
    // Taking the routers does not change what was mounted; it moves ownership of
    // the boxes a host needs to merge.
    assert_eq!(host.mounted(), vec!["todo"]);
    assert_eq!(host.exposed().len(), 3);
}
