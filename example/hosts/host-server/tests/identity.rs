//! The caller identity, followed from the surface that received it to the impl
//! that read it.
//!
//! `Ctx` in the contract is only worth anything if the *generated* adapters fill
//! it in: a parameter nobody populates is a task-local with extra steps. These
//! drive the generated clap tree and the generated axum route and assert on what
//! the impl saw, which is the whole claim.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use gabion::caps::{Caps, Http, HttpRequest};
use gabion::prelude::Ctx;
use todo_api::{CreateTodo, Todo, TodoErr, TodoId, TodoService};
use todo_core::TodoServiceImpl;

/// An impl that delegates to the real one and records the context each call
/// arrived with. The point is to observe the adapter's work, not to fake it.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Ctx>>>);

impl Seen {
    fn last(&self) -> Ctx {
        self.0.lock().unwrap().pop().expect("no call was recorded")
    }
}

struct Recording<C: Caps> {
    inner: TodoServiceImpl<C>,
    seen: Seen,
}

#[gabion::async_trait]
impl<C: Caps> TodoService for Recording<C> {
    async fn create(&self, ctx: &Ctx, input: CreateTodo) -> Result<Todo, TodoErr> {
        self.seen.0.lock().unwrap().push(ctx.clone());
        self.inner.create(ctx, input).await
    }

    async fn complete(&self, ctx: &Ctx, input: TodoId) -> Result<Todo, TodoErr> {
        self.seen.0.lock().unwrap().push(ctx.clone());
        self.inner.complete(ctx, input).await
    }

    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr> {
        self.seen.0.lock().unwrap().push(ctx.clone());
        self.inner.list(ctx, ()).await
    }
}

fn service() -> (Arc<dyn TodoService + Send + Sync>, Seen) {
    let seen = Seen::default();
    let svc = Recording {
        inner: TodoServiceImpl::new(gabion::testing::TestCaps::new()),
        seen: seen.clone(),
    };
    (Arc::new(svc), seen)
}

/// The CLI surface reads `--principal` / `--tenant` / `--trace` off the
/// generated command, so a local caller can name itself without a hand-written
/// adapter — and cannot silently be anonymous.
#[tokio::test]
async fn the_generated_cli_carries_the_caller_into_the_impl() {
    let (svc, seen) = service();
    let cmd = todo_api::__todo_cli::todo_cli_command();
    let matches = clap::Command::new("app")
        .subcommand(cmd)
        .try_get_matches_from([
            "app",
            "todo",
            "--principal",
            "agent-7",
            "--tenant",
            "acme",
            "create",
            "--json",
            r#"{"title":"milk"}"#,
        ])
        .unwrap();
    let (_, service_matches) = matches.subcommand().unwrap();
    todo_api::__todo_cli::todo_cli_dispatch(svc.as_ref(), &Ctx::anonymous(), service_matches)
        .await
        .unwrap();

    let ctx = seen.last();
    assert_eq!(ctx.principal(), Some("agent-7"));
    assert_eq!(ctx.tenant(), Some("acme"));
}

/// The client stamps the identity and the route reads it — the same `Ctx`
/// conversion on both sides, so an identity set by the caller is the identity the
/// impl sees. This is the whole generated pair, end to end, over a real socket.
#[tokio::test]
async fn the_generated_client_and_route_agree_on_the_caller() {
    let (svc, seen) = service();
    let app = todo_api::__todo_server::todo_router(svc);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = todo_api::TodoClient::new(base, gabion_native::ReqwestHttp::new());
    let ctx = Ctx::anonymous()
        .with_principal("agent-7")
        .with_tenant("acme")
        .with_trace("trace-1");
    let created = client
        .create(
            &ctx,
            CreateTodo {
                title: "milk".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(created.title, "milk");

    let seen = seen.last();
    assert_eq!(
        seen.principal(),
        Some("agent-7"),
        "the identity the client sent did not reach the impl"
    );
    assert_eq!(seen.tenant(), Some("acme"));
    assert_eq!(seen.trace(), Some("trace-1"));
}

/// The client is a `TodoService`, so a host can hand a renderer either the core
/// or a remote impl and the screen cannot tell. Proven by calling through the
/// trait object, which is exactly what `todo-render` holds.
#[tokio::test]
async fn the_client_can_stand_in_where_the_core_stood() {
    let (svc, _seen) = service();
    let app = todo_api::__todo_server::todo_router(svc);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let remote: Arc<dyn TodoService + Send + Sync> = Arc::new(todo_api::TodoClient::new(
        base,
        gabion_native::ReqwestHttp::new(),
    ));
    let listed = remote.list(&Ctx::anonymous(), ()).await.unwrap();
    assert!(listed.is_empty(), "a fresh store lists nothing");
}

/// A call with no headers is anonymous rather than invented: a route that made
/// up a principal would be worse than one that admits it has none.
#[tokio::test]
async fn a_route_call_without_headers_is_anonymous() {
    let (svc, seen) = service();
    let app = todo_api::__todo_server::todo_router(svc);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let request = HttpRequest::post(
        format!("{base}/api/todo.list"),
        serde_json::to_vec(&()).unwrap(),
    );
    let http = gabion_native::ReqwestHttp::new();
    let reply = http.send(request).await.unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(seen.last().principal(), None);
}
