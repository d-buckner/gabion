//! todo business logic. The whole point of the framework: this file is
//! runtime-free and wasm-safe. Effects come in via `Caps` only.
//!
//! Proof: `cargo build -p todo-core --target wasm32-unknown-unknown` passes.
//! Add `std::time::SystemTime::now()` anywhere here and it fails to compile.

use gabion::prelude::*;
use todo_api::{todo_err, CreateTodo, Todo, TodoErr, TodoError, TodoId, TodoService};

pub struct TodoServiceImpl<C: Caps> {
    caps: C,
}

impl<C: Caps> TodoServiceImpl<C> {
    pub fn new(caps: C) -> Self {
        Self { caps }
    }

    fn key(id: &str) -> String {
        format!("todo/{id}")
    }
}

fn internal() -> TodoErr {
    todo_err(TodoError::Internal)
}

#[async_trait]
impl<C: Caps> TodoService for TodoServiceImpl<C> {
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr> {
        if input.title.trim().is_empty() {
            return Err(todo_err(TodoError::EmptyTitle));
        }
        let now = self.caps.clock().now().await;
        let id = format!("t{}", now.0);
        let todo = Todo {
            id: id.clone(),
            title: input.title,
            done: false,
            created_at: now.0,
        };
        let json = gabion::serde_json::to_string(&todo).map_err(|_| internal())?;
        self.caps
            .store()
            .set(&Self::key(&id), &json)
            .await
            .map_err(|_| internal())?;
        Ok(todo)
    }

    async fn complete(&self, input: TodoId) -> Result<Todo, TodoErr> {
        let raw = self
            .caps
            .store()
            .get(&Self::key(&input.0))
            .await
            .map_err(|_| internal())?;
        let mut todo = raw
            .and_then(|s| gabion::serde_json::from_str::<Todo>(&s).ok())
            .ok_or_else(|| todo_err(TodoError::NotFound))?;
        todo.done = true;
        let json = gabion::serde_json::to_string(&todo).map_err(|_| internal())?;
        self.caps
            .store()
            .set(&Self::key(&todo.id), &json)
            .await
            .map_err(|_| internal())?;
        Ok(todo)
    }

    async fn list(&self, _input: ()) -> Result<Vec<Todo>, TodoErr> {
        let items = self
            .caps
            .store()
            .list("todo/")
            .await
            .map_err(|_| internal())?;
        items
            .iter()
            .map(|(_, v)| gabion::serde_json::from_str::<Todo>(v).map_err(|_| internal()))
            .collect()
    }

    async fn recompute_stats(&self, _input: ()) -> Result<(), TodoErr> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use gabion::testing::TestCaps;

    #[tokio::test]
    async fn create_list_complete() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let t = svc
            .create(CreateTodo {
                title: "milk".into(),
            })
            .await
            .unwrap();
        assert!(!t.done);
        assert_eq!(svc.list(()).await.unwrap().len(), 1);
        let done = svc.complete(TodoId(t.id.clone())).await.unwrap();
        assert!(done.done);
    }

    #[tokio::test]
    async fn empty_title_is_invalid_input() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let e = svc
            .create(CreateTodo { title: "  ".into() })
            .await
            .unwrap_err();
        assert_eq!(e.kind_code, ErrorKind::InvalidInput);
        assert_eq!(e.kind_code.http_status(), 400);
    }

    #[tokio::test]
    async fn complete_missing_is_404() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let e = svc.complete(TodoId("nope".into())).await.unwrap_err();
        assert_eq!(e.kind_code.http_status(), 404);
        assert_eq!(e.kind_code.exit_code(), 3);
    }
}
