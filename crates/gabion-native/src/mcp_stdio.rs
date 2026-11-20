//! MCP over stdio: the transport an agent's process supervisor uses.
//!
//! The protocol lives in `gabion::mcp`; this is the part that needs an operating
//! system — a reader, a writer, and a runtime to drive the async dispatch. It is
//! here rather than in the runtime because `gabion` must stay wasm-safe, and
//! threads and file descriptors are exactly what a browser does not have.
//!
//! Messages are newline-delimited JSON, one per line, as MCP's stdio transport
//! specifies. A notification produces no line at all: answering one is a protocol
//! error that a client reports as a broken stream.

use std::io::{BufRead, Write};

use gabion::mcp::Server;

/// Serve requests until the reader ends. Separated from [`serve_stdio`] so a test
/// can drive it with buffers instead of a process: the thing worth testing is the
/// framing, not `std::io`.
pub fn pump<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    server: &Server,
) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request = line.trim();
        if request.is_empty() {
            continue;
        }
        if let Some(reply) = runtime.block_on(server.handle(request)) {
            writeln!(output, "{reply}")?;
            // Flush per message: a client blocked on a reply it cannot see is
            // indistinguishable from a hung server.
            output.flush()?;
        }
    }
}

/// Serve this process's stdin/stdout as an MCP server.
///
/// One process, one client: that is what the stdio transport means, so there is
/// no accept loop and no framing beyond the newline.
pub fn serve_stdio(server: &Server) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    pump(stdin.lock(), stdout.lock(), server)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a failing assertion should show the protocol bytes
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A server whose one tool answers. The framing is what is under test here;
    /// `gabion::mcp`'s own tests cover the protocol.
    fn server() -> Server {
        use gabion::ctx::Ctx;
        use gabion::error::WireError;
        use gabion::mcp::{Tool, ToolResult};
        use serde_json::Value;

        struct Echo;

        impl gabion::mcp::Service for Echo {
            fn ns(&self) -> &str {
                "demo"
            }
            fn tools(&self) -> Vec<Tool> {
                vec![Tool {
                    name: "demo.echo".to_string(),
                    description: "Echo.".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                }]
            }
            fn call<'a>(&'a self, _ctx: &'a Ctx, _name: &'a str, args: Value) -> ToolResult<'a> {
                Box::pin(
                    async move { Ok::<Value, WireError>(serde_json::json!({ "echoed": args })) },
                )
            }
        }

        Server::new().with(std::sync::Arc::new(Echo))
    }

    #[test]
    fn one_line_in_one_line_out() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut output = Vec::new();
        pump(Cursor::new(input), &mut output, &server()).unwrap();
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one reply per request");
        let init: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert!(init["result"]["protocolVersion"].is_string());
        let list: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], "demo.echo");
    }

    /// A notification must not produce a line. A client that gets one for a
    /// notification has to decide whether it is a reply to something else.
    #[test]
    fn a_notification_produces_no_line() {
        let input = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let mut output = Vec::new();
        pump(Cursor::new(input), &mut output, &server()).unwrap();
        assert!(
            output.is_empty(),
            "got: {}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn a_blank_line_is_not_a_request() {
        let input = "\n\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n";
        let mut output = Vec::new();
        pump(Cursor::new(input), &mut output, &server()).unwrap();
        assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
    }
}
