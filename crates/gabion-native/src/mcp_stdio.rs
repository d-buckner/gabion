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
//!
//! Requests are dispatched **concurrently**, which is what makes
//! `notifications/cancelled` usable: a client that gives up on a long tool call
//! sends the notification while the call is still running, and a pump that answered
//! one request at a time would not read it until the call it was meant to stop had
//! finished. Replies may therefore arrive out of order — JSON-RPC matches them by
//! id, and a client that assumed order was already wrong.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use gabion::mcp::Server;

/// Serve requests until the reader ends.
///
/// Separated from [`serve_stdio`] so a test can drive it with buffers instead of a
/// process: the things worth testing are the framing and the concurrency, not
/// `std::io`.
pub fn pump<R, W>(mut input: R, output: W, server: Server) -> std::io::Result<()>
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let output = Arc::new(Mutex::new(output));
    let mut tasks = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let request = line.trim().to_string();
        if request.is_empty() {
            continue;
        }
        let server = server.clone();
        let output = Arc::clone(&output);
        tasks.push(runtime.spawn(async move {
            let mut sink = LineSink {
                output: Arc::clone(&output),
            };
            // A stream's notifications go out as they are produced; the answer
            // goes out last, so a client never sees the count before the items.
            if let Some(reply) = server.handle_with(&request, &mut sink).await {
                sink.write(&reply);
            }
        }));
    }
    // Let the calls that outlived the input finish writing before the runtime goes.
    runtime.block_on(async {
        for task in tasks {
            let _ = task.await;
        }
    });
    Ok(())
}

/// One message per line on the shared stdout.
///
/// The lock is taken per message, not per request: a stream's notifications have
/// to reach the client while its answer is still being produced, and a client that
/// reads a `tools/call` reply before the progress it names is reading a lie.
struct LineSink<W> {
    output: Arc<Mutex<W>>,
}

impl<W: Write> LineSink<W> {
    fn write(&self, line: &str) {
        let Ok(mut out) = self.output.lock() else {
            return;
        };
        // Write and flush per message: a client blocked on a reply it cannot
        // see is indistinguishable from a hung server.
        if writeln!(out, "{line}").is_ok() {
            let _ = out.flush();
        }
    }
}

impl<W: Write + Send> gabion::mcp::Emit for LineSink<W> {
    fn emit(&mut self, line: String) -> gabion::stream::BoxFuture<'_, ()> {
        self.write(&line);
        Box::pin(async {})
    }
}

/// Serve this process's stdin/stdout as an MCP server.
///
/// One process, one client: that is what the stdio transport means, so there is
/// no accept loop and no framing beyond the newline.
pub fn serve_stdio(server: Server) -> std::io::Result<()> {
    pump(std::io::stdin().lock(), std::io::stdout(), server)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a failing assertion should show the protocol bytes
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    /// A buffer the test can read after `pump` returns: the tasks own a clone, so
    /// the writer has to be shared rather than borrowed.
    #[derive(Clone, Default)]
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

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

    /// A tool that runs until cancelled — the case the concurrent pump exists for.
    fn slow_server() -> Server {
        use gabion::ctx::Ctx;
        use gabion::error::WireError;
        use gabion::mcp::{Tool, ToolResult};
        use serde_json::Value;

        struct Slow;

        impl gabion::mcp::Service for Slow {
            fn ns(&self) -> &str {
                "demo"
            }
            fn tools(&self) -> Vec<Tool> {
                vec![Tool {
                    name: "demo.slow".to_string(),
                    description: "Runs until cancelled.".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                }]
            }
            fn call<'a>(&'a self, ctx: &'a Ctx, _name: &'a str, _args: Value) -> ToolResult<'a> {
                Box::pin(async move {
                    let waited = tokio::time::timeout(
                        Duration::from_secs(5),
                        ctx.cancel_token().cancelled(),
                    );
                    Ok::<Value, WireError>(serde_json::json!({ "cancelled": waited.await.is_ok() }))
                })
            }
        }

        Server::new().with(std::sync::Arc::new(Slow))
    }

    #[test]
    fn one_line_in_one_line_out() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let output = SharedBuf::default();
        pump(Cursor::new(input), output.clone(), server()).unwrap();
        let text = output.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one reply per request");
        // Concurrent dispatch means the order is not guaranteed; JSON-RPC matches
        // replies by id, so the test does too.
        let mut replies: Vec<serde_json::Value> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        replies.sort_by_key(|r| r["id"].as_u64().unwrap_or(0));
        assert!(replies[0]["result"]["protocolVersion"].is_string());
        assert_eq!(replies[1]["result"]["tools"][0]["name"], "demo.echo");
    }

    /// A notification must not produce a line. A client that gets one for a
    /// notification has to decide whether it is a reply to something else.
    #[test]
    fn a_notification_produces_no_line() {
        let input = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let output = SharedBuf::default();
        pump(Cursor::new(input), output.clone(), server()).unwrap();
        assert!(output.text().is_empty(), "got: {}", output.text());
    }

    #[test]
    fn a_blank_line_is_not_a_request() {
        let input = "\n\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n";
        let output = SharedBuf::default();
        pump(Cursor::new(input), output.clone(), server()).unwrap();
        assert_eq!(output.text().lines().count(), 1);
    }

    /// The transport-level version of the cancellation test: a call and the
    /// notification that stops it arrive on the same stream, one line each, and the
    /// pump must be reading while the call is still running.
    #[test]
    fn a_cancellation_arrives_while_the_call_is_running() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"demo.slow\",\"arguments\":{}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n",
        );
        let output = SharedBuf::default();
        let started = std::time::Instant::now();
        pump(Cursor::new(input), output.clone(), slow_server()).unwrap();
        let text = output.text();
        let reply: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let observed: serde_json::Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            observed["cancelled"], true,
            "the tool saw no cancellation: {reply}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the call ran to its own timeout instead of being cancelled: {:?}",
            started.elapsed()
        );
    }
}
