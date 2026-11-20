//! todo UI code. It can name `TodoService` (the trait) and the wire types —
//! and nothing from todo-core. The host injects the concrete impl,
//! so this crate compiles identically for native and wasm.
//!
//! The GUI is Dioxus: core + signals only, no renderer feature. The host
//! picks the renderer (host-native: dioxus-desktop, host-web: dioxus-web)
//! and injects the service via launch context:
//!
//! ```ignore
//! dioxus::LaunchBuilder::new()
//!     .with_context(todo_render::Svc(svc))
//!     .launch(todo_render::TodoApp);
//! ```

use std::sync::Arc;

use dioxus::prelude::*;
use gabion::prelude::Ctx;
use todo_api::{CreateTodo, Todo, TodoErr, TodoId, TodoService};
use ui_shared::{Empty, Failure, Heading, Surface};

/// Service handle injected by the host through the launch context
/// (`LaunchBuilder::with_context`). The UI only ever sees the trait — and the
/// caller identity the host decided on, which travels with the handle so no
/// screen has to invent one.
#[derive(Clone)]
pub struct Svc {
    pub inner: Arc<dyn TodoService + Send + Sync>,
    pub ctx: Ctx,
}

impl Svc {
    /// A screen acting on its own behalf, which is what a local desktop app is.
    #[must_use]
    pub fn new(inner: Arc<dyn TodoService + Send + Sync>) -> Self {
        Self {
            inner,
            ctx: Ctx::anonymous(),
        }
    }

    /// A screen acting for a caller the host authenticated.
    #[must_use]
    pub fn with_ctx(inner: Arc<dyn TodoService + Send + Sync>, ctx: Ctx) -> Self {
        Self { inner, ctx }
    }
}

/// The todo list, as a component another screen may mount.
///
/// Props only: the items as data and one callback for the checkbox. It does not
/// take a service handle, so a crate composing this panel inherits one function
/// signature and the `*-api` types — not `Svc`, not the trait, and not the store
/// behind them.
///
/// `pub(crate)` today, because nothing outside composes it yet: a component is
/// published when something uses it, and `over-broad-visibility` is the rule that
/// will ask for the `pub` when that happens.
///
/// ```ignore
/// rsx! { TodoPanel { items, on_toggle } }
/// ```
#[component]
pub(crate) fn TodoPanel(items: Vec<Todo>, on_toggle: EventHandler<TodoId>) -> Element {
    if items.is_empty() {
        return rsx! { Empty { note: "nothing to do".to_string() } };
    }
    rsx! {
        ul {
            for t in items {
                li {
                    key: "{t.id}",
                    class: if t.done { "done" } else { "" },
                    input {
                        r#type: "checkbox",
                        checked: t.done,
                        onchange: {
                            let id = t.id.clone();
                            move |_| on_toggle.call(TodoId(id.clone()))
                        },
                    },
                    span { "{t.title}" }
                }
            }
        }
    }
}

/// The todo screen: reads the injected [`Svc`] from context, owns the form and the
/// banners, and renders [`TodoPanel`] for the list.
#[component]
pub fn TodoApp() -> Element {
    let svc = use_context::<Svc>();
    let mut title = use_signal(String::new);
    let todos = use_resource({
        let svc = svc.clone();
        move || {
            let svc = svc.clone();
            async move { svc.inner.list(&svc.ctx, ()).await.map_err(|e| e.message) }
        }
    });

    // Reading the resource in the body subscribes this scope: the component
    // re-renders whenever the list refetches. The match yields owned values,
    // so nothing borrows outlives the statement.
    let toggle = EventHandler::new({
        let svc = svc.clone();
        move |id: TodoId| {
            let svc = svc.clone();
            spawn(async move {
                let _ = svc.inner.complete(&svc.ctx, id).await;
                refresh(todos);
            });
        }
    });

    // Reading the resource in the body subscribes this scope: the component
    // re-renders whenever the list refetches. The match yields owned values,
    // so nothing borrows outlives the statement.
    let list_view = match todos() {
        None => rsx! { Empty { note: "loading…".to_string() } },
        Some(Err(e)) => rsx! { Failure { message: e } },
        Some(Ok(items)) => rsx! { TodoPanel { items, on_toggle: toggle } },
    };

    rsx! {
        style { dangerous_inner_html: ui_shared::CSS }
        Surface {
            Heading { title: "todo".to_string(), subtitle: None, actions: None }
            form {
                class: "gb",
                onsubmit: {
                    let svc = svc.clone();
                    move |evt| {
                        evt.prevent_default();
                        let text = title.cloned();
                        if !text.trim().is_empty() {
                            let svc = svc.clone();
                            spawn(async move {
                                let _ = svc.inner.create(&svc.ctx, CreateTodo { title: text }).await;
                                refresh(todos);
                            });
                        }
                        title.set(String::new());
                    }
                },
                input {
                    r#type: "text",
                    value: "{title}",
                    placeholder: "what needs doing?",
                    oninput: move |evt| title.set(evt.value()),
                },
                button { r#type: "submit", class: "primary", "add" }
            }
            {list_view}
        }
    }
}

// --- CLI string renderer (host-native's `ui` subcommand) -------------------

/// `Resource` is `Copy`, so restart it by value: `.clone()` on a `Copy` type is
/// what clippy rejects, and `let todos = todos;` is what `redundant_locals`
/// rejects. Taking it by value is the honest form of "copy it, restart the copy".
fn refresh(mut todos: Resource<Result<Vec<Todo>, String>>) {
    todos.restart();
}

/// A "view": renders the list via the trait object it was given.
pub async fn render_list(
    svc: &(dyn TodoService + Send + Sync),
    ctx: &Ctx,
) -> Result<String, TodoErr> {
    let todos = svc.list(ctx, ()).await?;
    let mut out = String::new();
    for t in todos {
        let mark = if t.done { "x" } else { " " };
        out.push_str(&format!("[{}] {}\n", mark, t.title));
    }
    Ok(out)
}

/// A "controller": user action -> service call, for a caller that does not hold a
/// component's props. Same code native and wasm.
pub async fn on_add_clicked(
    svc: &(dyn TodoService + Send + Sync),
    ctx: &Ctx,
    title: &str,
) -> Result<Todo, TodoErr> {
    svc.create(
        ctx,
        CreateTodo {
            title: title.to_string(),
        },
    )
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // a failing assertion should show the markup
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // a snapshot test reads an env var and writes a file
mod snapshot {
    //! The published component, rendered to HTML and compared against a committed
    //! file.
    //!
    //! This is the UI counterpart of `openapi_snapshot.rs`: a component can keep its
    //! props and change what it shows, and nothing on the wire would notice. The
    //! snapshot makes that a diff someone reads.
    //!
    //! Bless with `GABION_BLESS_UI=1 cargo test -p todo-render`.

    use super::*;

    /// The panel with fixture data, as a component: props are built inside a
    /// runtime, which is where Dioxus allocates them.
    #[component]
    fn Fixture() -> Element {
        let items = vec![
            Todo {
                id: "t1".into(),
                title: "buy sandspice".into(),
                done: false,
                created_at: 1_700_000_000,
            },
            Todo {
                id: "t2".into(),
                title: "ship it".into(),
                done: true,
                created_at: 1_700_000_001,
            },
        ];
        // The handler is a prop, and SSR renders the markup a handler cannot change,
        // so a no-op is the honest fixture: this snapshots what the panel *shows*.
        rsx! {
            TodoPanel { items, on_toggle: move |_id: TodoId| {} }
        }
    }

    fn html() -> String {
        let mut dom = VirtualDom::new(Fixture);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../.gabion/ui/todo-panel.html")
    }

    #[test]
    fn the_panel_markup_matches_the_committed_snapshot() {
        let rendered = html();
        let path = path();
        if std::env::var("GABION_BLESS_UI").is_ok() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &rendered).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "no snapshot at {}: {e}. Run `GABION_BLESS_UI=1 cargo test -p todo-render`,                  then read the diff.",
                path.display()
            )
        });
        assert_eq!(
            rendered, committed,
            "the panel markup changed. If it should have, re-bless and review the diff."
        );
    }
}
