//! The stream surface: one WebSocket per host, multiplexed by feature namespace
//! (`docs/streaming.md` §5, §7).
//!
//! One endpoint, `/api/ws`, mounted the way `/mcp` is. Every frame names an
//! operation the way a route, an MCP tool and a CLI subcommand name it, and the
//! namespace in that name is the isolation unit this module enforces.
//!
//! # What the pump guarantees
//!
//! - **Namespace wall (§5).** A `call` or `close` may only reach a stream this
//!   connection already opened, and an op must belong to that stream's namespace.
//!   One comparison against the open-stream registry, at the one place that has it,
//!   and the record leaves the registry when the stream stops — a namespace whose
//!   last stream is over is closed to `call` frames again.
//! - **A refusal names its kind (§5).** The site that knows what went wrong
//!   picks the `ErrorKind`, so an unknown op is `NotFound` here as it is over HTTP,
//!   MCP and the CLI, and only a namespace nobody opened is `Forbidden`.
//! - **Ordering (§7.1).** `call` frames for one namespace dispatch in arrival
//!   order. Different namespaces run concurrently, so one slow op cannot hold up
//!   another feature — that is the bug the MCP stdio pump was rebuilt to avoid.
//! - **`close` is out of band.** It is handled in the read loop, never queued
//!   behind a call that is still in flight.
//! - **Policy is the signature's (§7.2, §7.3).** `Each` awaits every write, so a
//!   slow subscriber backpressures the connection. `Latest` holds one slot and
//!   replaces an untaken value, so it cannot.
//! - **An end is stated (§7.4).** Every stream leaves through `end` or `error`. A
//!   dropped socket cancels its producers rather than letting them run on.

mod pump;
mod registry;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use futures_util::{SinkExt, StreamExt};
use gabion::ctx::{Cancel, Ctx};
use gabion::error::ErrorKind;
use gabion::frames::{Frame, StreamId};
use gabion::ws::WsService;

use pump::{fail, no_such_op, pump};
use registry::{locked, Call, Registry};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// How many frames may sit unwritten before a producer waits.
///
/// Bounded on purpose: an unbounded queue would turn an `Each` stream's backpressure
/// into memory growth, which is the one thing §5's documented limit says gabion
/// will not do.
const WRITE_QUEUE: usize = 64;

/// `GET /api/ws`, serving every mounted service that has a stream surface.
///
/// Mounted once per host, not once per op. A host with no streaming feature gets
/// no route at all, rather than one that answers nothing.
pub fn ws_router(services: Vec<Arc<dyn WsService>>) -> axum::Router {
    if services.is_empty() {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/ws", axum::routing::get(upgrade))
        .with_state(Arc::new(Surface { services }))
}

#[derive(Clone)]
pub(super) struct Surface {
    services: Vec<Arc<dyn WsService>>,
}

impl Surface {
    pub(super) fn find(&self, op: &str) -> Option<&Arc<dyn WsService>> {
        self.services.iter().find(|s| s.serves(op))
    }
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(surface): State<Arc<Surface>>,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| serve(socket, surface))
}

/// One connection: a writer, a reader, and the streams it has opened.
async fn serve(socket: WebSocket, surface: Arc<Surface>) {
    let (sink, source) = socket.split();
    let (out, mut queue) = mpsc::channel::<Frame>(WRITE_QUEUE);
    let registry: Arc<Mutex<Registry>> = Arc::new(Mutex::new(Registry::default()));

    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(frame) = queue.recv().await {
            let Ok(text) = gabion::serde_json::to_string(&frame) else {
                let _ = sink.send(Message::Close(None)).await;
                return;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
    });

    let mut source = source;
    loop {
        match source.next().await {
            Some(Ok(Message::Text(text))) => {
                let frame = match gabion::serde_json::from_str::<Frame>(&text) {
                    Ok(frame) => frame,
                    Err(e) => {
                        // `StreamId(0)` is the no-stream id: the id the client would
                        // have named is exactly what failed to parse, and `0` is the
                        // one number a client never opens a stream under.
                        fail(
                            &out,
                            StreamId(0),
                            ErrorKind::InvalidInput,
                            &format!("frame would not parse: {e}"),
                        )
                        .await;
                        break;
                    }
                };
                let conn = Conn {
                    surface: &surface,
                    registry: &registry,
                    out: &out,
                };
                if !read_one(frame, &conn).await {
                    break;
                }
            }
            // Ping, pong and binary are not gabion's framing; the socket itself
            // keeps them straight, so they are ignored rather than refused.
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => break,
        }
    }

    // The socket is gone. Every producer on it is cancelled, including one that is
    // idle and would otherwise sit on a stream nobody is reading (§7.4).
    locked(&registry).cancel_all();
    drop(out);
    let _ = writer.await;
}

/// Everything one frame handler needs about the connection it came in on.
pub(super) struct Conn<'a> {
    pub(super) surface: &'a Arc<Surface>,
    pub(super) registry: &'a Arc<Mutex<Registry>>,
    pub(super) out: &'a mpsc::Sender<Frame>,
}

/// Handle one client frame. `false` means the connection is over.
async fn read_one(frame: Frame, conn: &Conn<'_>) -> bool {
    let Conn { registry, out, .. } = *conn;
    match frame {
        Frame::Open {
            id,
            op,
            input,
            meta,
        } => {
            open(OpenRequest {
                id,
                op,
                input,
                ctx: Ctx::from_meta(&meta),
                conn,
            })
            .await;
            true
        }
        Frame::Call {
            id,
            op,
            input,
            meta,
        } => {
            call(id, op, input, &meta, conn).await;
            true
        }
        Frame::Close { id } => {
            // Out of band: never queued behind a call still in flight.
            if !locked(registry).close(id) {
                fail(
                    out,
                    id,
                    ErrorKind::NotFound,
                    &format!("stream {id} was not opened on this connection"),
                )
                .await;
            }
            true
        }
        other => {
            fail(
                out,
                other.id(),
                ErrorKind::InvalidInput,
                &format!("{other:?} is not a frame a client sends"),
            )
            .await;
            true
        }
    }
}

/// One `open` frame, resolved against the mounted services.
struct OpenRequest<'a> {
    id: StreamId,
    op: String,
    input: Value,
    ctx: Ctx,
    conn: &'a Conn<'a>,
}

async fn open(request: OpenRequest<'_>) {
    let OpenRequest {
        id,
        op,
        input,
        ctx,
        conn,
    } = request;
    let Conn {
        surface,
        registry,
        out,
    } = *conn;
    let Some(service) = surface.find(&op) else {
        let _ = out.send(no_such_op(id, &op)).await;
        return;
    };
    let ns = service.ns().to_string();
    let cancel = Cancel::new();
    if !locked(registry).claim(id, ns.clone(), cancel.clone()) {
        fail(
            out,
            id,
            ErrorKind::Conflict,
            &format!("stream {id} is already open on this connection"),
        )
        .await;
        return;
    }
    let ctx = ctx.with_cancel(cancel);
    let started = service.open(&ctx, &op, input).await;
    let opened = match started {
        Ok(opened) => opened,
        Err(error) => {
            let _ = out.send(Frame::Error { id, error }).await;
            locked(registry).retire(id);
            return;
        }
    };
    tokio::spawn(pump(id, ns, opened, ctx, out.clone(), Arc::clone(registry)));
}

/// One ordinary op on a stream this connection already opened (§5).
///
/// The namespace wall is one lookup here and nowhere else: the queue a call joins
/// is the queue of the namespace with a live stream on this connection. A call that
/// names a namespace with no live stream has no queue, so it is refused rather than
/// run — and the refusal names that namespace, because "that stream is over" and
/// "you never opened this" are different answers a client acts on differently.
async fn call(id: StreamId, op: String, input: Value, meta: &Value, conn: &Conn<'_>) {
    let Conn { registry, out, .. } = *conn;
    let Some(ns) = Frame::op_namespace(&op) else {
        // A name with no namespace cannot address any stream, which the client's own
        // check calls `InvalidInput`; the server must not answer it differently.
        let why = format!("`{op}` is not a namespaced op");
        fail(out, id, ErrorKind::InvalidInput, &why).await;
        return;
    };
    let ns = ns.to_string();
    let sender = locked(registry).sender_for(&ns, conn);
    match sender {
        Some(sender) => {
            let _ = sender.send(Call {
                op,
                input,
                ctx: Ctx::from_meta(meta),
                id,
            });
        }
        None => {
            fail(
                out,
                id,
                ErrorKind::Forbidden,
                &format!(
                    "`{op}` is outside every namespace this connection opened: \
                     no stream in `{ns}` is live"
                ),
            )
            .await;
        }
    }
}
