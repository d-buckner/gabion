//! Server host: one line of real work — mount the generated router.

use std::sync::Arc;
use todo_core::TodoServiceImpl;

#[tokio::main]
#[allow(clippy::unwrap_used)] // spec bans unwrap in *-core; host main is bootstrap
async fn main() {
    let caps = sietch_native::NativeCaps::new();
    let svc = Arc::new(TodoServiceImpl::new(caps));

    let app = todo_api::__todo_server::todo_router(svc);

    let addr = "127.0.0.1:3456";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
