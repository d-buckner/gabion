//! Server host bootstrap: pick a port, serve the app — or, for an agent's
//! process supervisor, serve the same tools over stdio instead.
//!
//! `--mcp-stdio` is the difference between "a human can call this" and "an agent
//! is talking to it": the protocol is identical, only the transport changes, and
//! both come from the one `gabion::mcp::Server` this host builds.

// Hosts are effect homes: reading argv and owning the process is the job.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

#[tokio::main]
#[allow(clippy::unwrap_used)] // spec bans unwrap in *-core; host main is bootstrap
async fn main() {
    if std::env::args().any(|a| a == "--mcp-stdio") {
        let (checks, incidents) = host_pulse_server::services();
        let server = host_pulse_server::mcp(checks, incidents);
        if let Err(e) = gabion_native::mcp_stdio::serve_stdio(&server) {
            eprintln!("mcp stdio: {e}");
            std::process::exit(1);
        }
        return;
    }

    let addr = "127.0.0.1:3457";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("listening on http://{addr}");
    println!("  POST {addr}/api/check.add          (one route per operation)");
    println!("  POST {addr}/mcp                    (MCP over HTTP)");
    println!("  GET  {addr}/openapi.json           (both contracts)");
    println!("  --mcp-stdio                        (MCP over stdio instead)");
    axum::serve(listener, host_pulse_server::app())
        .await
        .unwrap();
}
