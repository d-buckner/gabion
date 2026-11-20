//! todo business logic. The whole point of the framework: this file is
//! runtime-free and wasm-safe. Effects come in via `Caps` only.
//!
//! Proof: `cargo build -p todo-core --target wasm32-unknown-unknown` passes.
//! Add `std::time::SystemTime::now()` anywhere here and it fails to compile.

use gabion::prelude::*;
use todo_api::{
    todo_err, CreateTodo, Todo, TodoErr, TodoError, TodoId, TodoInternals, TodoService,
};

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

    /// Short unpredictable suffix for ids. The clock alone is not an id: two
    /// creates in the same second used to collide, and the second silently
    /// overwrote the first.
    async fn nonce(&self) -> String {
        format!("{:08x}", self.caps.random().next_u64().await as u32)
    }
}

fn internal() -> TodoErr {
    todo_err(TodoError::Internal)
}

#[async_trait]
impl<C: Caps> TodoService for TodoServiceImpl<C> {
    async fn create(&self, ctx: &Ctx, input: CreateTodo) -> Result<Todo, TodoErr> {
        if input.title.trim().is_empty() {
            return Err(todo_err(TodoError::EmptyTitle));
        }
        let now = self.caps.clock().now().await;
        let id = format!("t{}-{}", now.0, self.nonce().await);
        let todo = Todo {
            id: id.clone(),
            title: input.title,
            done: false,
            created_at: now.0,
        };
        let json = gabion::serde_json::to_string(&todo).map_err(|_| internal())?;
        self.caps
            .store()
            .set_as(ctx, &Self::key(&id), &json)
            .await
            .map_err(|_| internal())?;
        Ok(todo)
    }

    async fn complete(&self, _ctx: &Ctx, input: TodoId) -> Result<Todo, TodoErr> {
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

    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr> {
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
}

/// In-proc only: `todo-api` exposes this trait so another feature can call it,
/// but `#[service]` never sees it, so it reaches no surface.
#[async_trait]
impl<C: Caps> TodoInternals for TodoServiceImpl<C> {
    async fn recompute_stats(&self) -> Result<(), TodoErr> {
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
            .create(
                &Ctx::anonymous(),
                CreateTodo {
                    title: "milk".into(),
                },
            )
            .await
            .unwrap();
        assert!(!t.done);
        assert_eq!(svc.list(&Ctx::anonymous(), ()).await.unwrap().len(), 1);
        let done = svc
            .complete(&Ctx::anonymous(), TodoId(t.id.clone()))
            .await
            .unwrap();
        assert!(done.done);
    }

    #[tokio::test]
    async fn empty_title_is_invalid_input() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let e = svc
            .create(&Ctx::anonymous(), CreateTodo { title: "  ".into() })
            .await
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert_eq!(e.kind.http_status(), 400);
    }

    #[tokio::test]
    async fn complete_missing_is_404() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let e = svc
            .complete(&Ctx::anonymous(), TodoId("nope".into()))
            .await
            .unwrap_err();
        assert_eq!(e.kind.http_status(), 404);
        assert_eq!(e.kind.exit_code(), 3);
    }

    /// The clock is frozen, so every create lands in the same second — exactly
    /// the case that used to overwrite the first record with the second, in
    /// silence.
    #[tokio::test]
    async fn two_creates_in_the_same_second_do_not_collide() {
        let svc = TodoServiceImpl::new(TestCaps::new());
        let a = svc
            .create(&Ctx::anonymous(), CreateTodo { title: "a".into() })
            .await
            .unwrap();
        let b = svc
            .create(&Ctx::anonymous(), CreateTodo { title: "b".into() })
            .await
            .unwrap();
        assert_ne!(a.id, b.id, "ids collided within one clock tick");
        assert_eq!(
            svc.list(&Ctx::anonymous(), ()).await.unwrap().len(),
            2,
            "a record was silently overwritten"
        );
    }

    /// The caller reaches the storage layer: this is the half of `Ctx` that no
    /// surface could prove, because it is about what an *effect* may record.
    #[tokio::test]
    async fn a_write_records_who_made_it() {
        let caps = TestCaps::new();
        let svc = TodoServiceImpl::new(caps.clone());
        let ctx = Ctx::anonymous().with_principal("agent-7");
        let todo = svc
            .create(
                &ctx,
                CreateTodo {
                    title: "milk".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            caps.store()
                .written_by(&TodoServiceImpl::<TestCaps>::key(&todo.id)),
            Some("agent-7".to_string()),
            "the principal the surface delivered must reach the store"
        );
    }

    /// A write with no caller records nothing, rather than an empty string: the
    /// absence of an identity must not look like one.
    #[tokio::test]
    async fn an_anonymous_write_records_no_writer() {
        let caps = TestCaps::new();
        let svc = TodoServiceImpl::new(caps.clone());
        let todo = svc
            .create(
                &Ctx::anonymous(),
                CreateTodo {
                    title: "milk".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            caps.store()
                .written_by(&TodoServiceImpl::<TestCaps>::key(&todo.id)),
            None
        );
    }

    /// Determinism is the other half of the capability: same seed, same ids,
    /// so a failing test stays failing.
    #[tokio::test]
    async fn ids_are_reproducible_under_test_caps() {
        let make = || async {
            TodoServiceImpl::new(TestCaps::new())
                .create(&Ctx::anonymous(), CreateTodo { title: "x".into() })
                .await
                .unwrap()
                .id
        };
        assert_eq!(make().await, make().await);
    }
}
