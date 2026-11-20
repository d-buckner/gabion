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

const CSS: &str = r#"
body { font-family: ui-sans-serif, system-ui, sans-serif; background: #101014;
       color: #e8e6e3; margin: 0; }
main { max-width: 34rem; margin: 3rem auto; padding: 0 1rem; }
h1 { font-size: 1.4rem; font-weight: 600; letter-spacing: .01em; }
form { display: flex; gap: .5rem; margin-bottom: 1.25rem; }
input[type=text] { flex: 1; padding: .55rem .7rem; border-radius: .4rem;
       border: 1px solid #2c2c34; background: #1a1a20; color: inherit; }
button { padding: .55rem .9rem; border-radius: .4rem; border: 1px solid #3a3a44;
       background: #26262e; color: inherit; cursor: pointer; }
ul { list-style: none; padding: 0; margin: 0; }
li { display: flex; align-items: center; gap: .6rem; padding: .5rem .2rem;
       border-bottom: 1px solid #23232b; }
li.done span { text-decoration: line-through; opacity: .5; }
.err { color: #e06c75; }
.muted { opacity: .5; font-size: .85rem; }
"#;

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

/// The todo screen. Reads the injected [`Svc`] from context.
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
    let list_view = match todos() {
        None => rsx! { p { class: "muted", "loading…" } },
        Some(Err(e)) => rsx! { p { class: "err", "{e}" } },
        Some(Ok(items)) => rsx! {
            if items.is_empty() {
                p { class: "muted", "nothing to do" }
            }
            ul {
                for t in items {
                    li {
                        key: "{t.id}",
                        class: if t.done { "done" } else { "" },
                        input {
                            r#type: "checkbox",
                            checked: t.done,
                            onchange: {
                                let svc = svc.clone();
                                let id = t.id.clone();
                                move |_| {
                                    let svc = svc.clone();
                                    let id = id.clone();
                                    spawn(async move {
                                        let _ = svc.inner.complete(&svc.ctx, TodoId(id)).await;
                                        refresh(todos);
                                    });
                                }
                            },
                        },
                        span { "{t.title}" }
                    }
                }
            }
        },
    };

    rsx! {
        style { dangerous_inner_html: CSS }
        main {
            h1 { "todo" }
            form {
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
                button { r#type: "submit", "add" }
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
