//! Server host bootstrap: pick a port, serve the app.

#[tokio::main]
#[allow(clippy::unwrap_used)] // spec bans unwrap in *-core; host main is bootstrap
async fn main() {
    let addr = "127.0.0.1:3457";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("listening on http://{addr}");
    axum::serve(listener, host_pulse_server::app())
        .await
        .unwrap();
}
