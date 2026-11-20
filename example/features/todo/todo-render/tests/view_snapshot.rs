//! The render crate's published view, snapshotted.
//!
//! The wire has two artifacts against *semantic* drift — the OpenAPI snapshot and
//! the surface lock's signature hash — and the UI had none: a component could keep
//! its props and change what it shows, and nothing noticed. This is the smallest
//! version of the same idea for the UI layer: render a fixed input and compare the
//! text against a committed file.
//!
//! Bless with `GABION_BLESS_UI=1 cargo test -p todo-render`, then read the diff:
//! that read is the whole value, exactly as it is for the OpenAPI document.
#![allow(clippy::unwrap_used, clippy::expect_used)]
// A snapshot test reads one env var and writes one file: an integration test is an
// effect home, and `gabion check` audits that the allow stays in one.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::PathBuf;

use gabion::prelude::{async_trait, Ctx};
use todo_api::{Todo, TodoErr, TodoId, TodoService};

/// A fixed list: the snapshot is about what the renderer does with data, not about
/// where the data came from. `todo-render` cannot reach `todo-core` (that is the
/// process wall), so the contract is implemented here and nothing else.
struct Fixture(Vec<Todo>);

#[async_trait]
impl TodoService for Fixture {
    async fn create(&self, _ctx: &Ctx, _input: todo_api::CreateTodo) -> Result<Todo, TodoErr> {
        unreachable!("the snapshot only renders a list")
    }

    async fn complete(&self, _ctx: &Ctx, _input: TodoId) -> Result<Todo, TodoErr> {
        unreachable!("the snapshot only renders a list")
    }

    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr> {
        Ok(self.0.clone())
    }
}

fn todo(id: &str, title: &str, done: bool) -> Todo {
    Todo {
        id: id.into(),
        title: title.into(),
        done,
        created_at: 1_700_000_000,
    }
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../.gabion/ui/todo-list.txt")
}

#[tokio::test]
async fn the_list_view_matches_the_committed_snapshot() {
    let svc = Fixture(vec![
        todo("t1", "buy sandspice", false),
        todo("t2", "ship it", true),
    ]);
    let rendered = todo_render::render_list(&svc, &Ctx::anonymous())
        .await
        .expect("the fixture answers");

    let path = snapshot_path();
    if std::env::var("GABION_BLESS_UI").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &rendered).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "no snapshot at {}: {e}. Run `GABION_BLESS_UI=1 cargo test -p todo-render`, \
             then read the diff.",
            path.display()
        )
    });
    assert_eq!(
        rendered, committed,
        "the rendered list changed. If it should have, re-bless and review the diff; \
         this file is the UI counterpart of the OpenAPI snapshot."
    );
}
