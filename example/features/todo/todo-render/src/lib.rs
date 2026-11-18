//! todo UI code. It can name `TodoService` (the trait) and the wire types —
//! and nothing from todo-core. The concrete impl is injected by the host,
//! so this crate compiles identically for native and wasm.

use todo_api::{CreateTodo, Todo, TodoErr, TodoId, TodoService};

/// A "view": renders the list via the trait object it was given.
pub async fn render_list(svc: &(dyn TodoService + Send + Sync)) -> Result<String, TodoErr> {
    let todos = svc.list(()).await?;
    let mut out = String::new();
    for t in todos {
        let mark = if t.done { "x" } else { " " };
        out.push_str(&format!("[{}] {}\n", mark, t.title));
    }
    Ok(out)
}

/// A "controller": user action -> service call. Same code native and wasm.
pub async fn on_add_clicked(
    svc: &(dyn TodoService + Send + Sync),
    title: &str,
) -> Result<Todo, TodoErr> {
    svc.create(CreateTodo { title: title.to_string() }).await
}

pub async fn on_checkbox_clicked(
    svc: &(dyn TodoService + Send + Sync),
    id: &str,
) -> Result<Todo, TodoErr> {
    svc.complete(TodoId(id.to_string())).await
}

