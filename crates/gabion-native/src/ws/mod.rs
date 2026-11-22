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
//!   behind a call that is still in flight, nor behind an `open` still accepting:
//!   a stream's record is claimed before its op starts, and the start runs in a
//!   task of its own, so a `close` reaches the registry while a slow producer is
//!   still deciding. Nothing in the read loop waits on the write queue either, so
//!   a stalled writer cannot keep it from reaching that `close`.
//! - **Policy is the signature's (§7.2, §7.3).** `Each` awaits every write, so a
//!   slow subscriber backpressures the connection. `Latest` holds one slot and
//!   replaces an untaken value, so it cannot.
//! - **Every queue on this path is bounded (§5).** `WRITE_QUEUE` frames may sit
//!   unwritten and `CALL_QUEUE` calls may wait for one namespace's queue; a producer
//!   waits out the first, and the read loop refuses the overflow rather than growing
//!   either one. The limits are `64` each, and §5 says so.
//! - **An end is stated (§7.4).** Every stream leaves through `end` or `error`. A
//!   dropped socket cancels its producers rather than letting them run on.

mod pump;
mod registry;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use futures_util::{SinkExt, StreamExt};
use gabion::ctx::{Cancel, Ctx};
use gabion::error::{ErrorKind, WireError};
use gabion::frames::{Frame, StreamId};
use gabion::ws::WsService;

use pump::{fail, no_such_op, pump, refuse};
use registry::{locked, Call, Registry, CALL_QUEUE};
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
                        );
                        break;
                    }
                };
                let conn = Conn {
                    surface: &surface,
                    registry: &registry,
                    out: &out,
                };
                if !read_one(frame, &conn) {
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
    // idle and would otherwise sit on a stream nobody is reading (§7.4), and
    // including a start that has not returned yet: `cancel_all` cancels the token
    // claimed before it ran. The reader's own sender goes next, but the writer is
    // awaited rather than dropped, so a task that was already holding a frame — a
    // retry, a pump's terminator — still gets its turn on the queue: the connection
    // closes when the last sender is gone, not when this loop stops reading.
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
///
/// Nothing here awaits: the one reader on the connection is also the only place a
/// `close` is handled, so anything it waited for — an op still accepting, a write
/// queue with no room — would be a producer that cannot be cancelled.
fn read_one(frame: Frame, conn: &Conn<'_>) -> bool {
    let Conn { registry, out, .. } = *conn;
    match frame {
        Frame::Open {
            id,
            op,
            input,
            meta,
        } => {
            start(id, op, input, &meta, conn);
            true
        }
        Frame::Call {
            id,
            op,
            input,
            meta,
        } => {
            call(id, op, input, &meta, conn);
            true
        }
        Frame::Close { id } => {
            // Out of band: never queued behind a call still in flight, and never
            // behind an `open` still starting — the record it removes is claimed
            // before that start runs.
            if !locked(registry).close(id) {
                fail(
                    out,
                    id,
                    ErrorKind::NotFound,
                    &format!("stream {id} was not opened on this connection"),
                );
            }
            true
        }
        other => {
            fail(
                out,
                other.id(),
                ErrorKind::InvalidInput,
                &format!("{other:?} is not a frame a client sends"),
            );
            true
        }
    }
}

/// Claim one `open`'s id, then start it in its own task.
///
/// The claim is synchronous and comes first on purpose. A `close` is answered from
/// the registry, so a stream must be in it before its op has had a chance to run:
/// otherwise the one producer the out-of-band rule exists for — the one still
/// reading its store, listing its targets, subscribing upstream — is the one a
/// `close` cannot find and cannot cancel. The start then does what `pump` does: own
/// its frames, and answer its own failure.
fn start(id: StreamId, op: String, input: Value, meta: &Value, conn: &Conn<'_>) {
    let Conn {
        surface,
        registry,
        out,
    } = *conn;
    let Some(service) = surface.find(&op) else {
        refuse(out, no_such_op(id, &op));
        return;
    };
    let ns = service.ns().to_string();
    let cancel = Cancel::new();
    if !locked(registry).claim(id, ns.clone(), cancel.clone()) {
        let error = WireError::new(
            ErrorKind::Conflict,
            format!("stream {id} is already open on this connection"),
        );
        refuse(out, Frame::Error { id, error });
        return;
    }
    let service = Arc::clone(service);
    let ctx = Ctx::from_meta(meta).with_cancel(cancel);
    let registry = Arc::clone(registry);
    let out = out.clone();
    tokio::spawn(async move {
        let opened = match service.open(&ctx, &op, input).await {
            Ok(opened) => opened,
            Err(error) => {
                // A start that fails still owes its id back: the refusal is the
                // answer to the `open`, and leaving the record behind would keep the
                // namespace wall open for an id no stream is behind.
                let _ = out.send(Frame::Error { id, error }).await;
                locked(&registry).retire(id);
                return;
            }
        };
        // Even a start that was closed or cancelled while it ran gets its pump: the
        // terminator is the answer to that `close`, and the token it already holds
        // ends it immediately.
        tokio::spawn(pump(id, ns, opened, ctx, out, registry));
    });
}

/// One ordinary op on a stream this connection already opened (§5).
///
/// The namespace wall is one lookup here and nowhere else: the queue a call joins
/// is the queue of the namespace with a live stream on this connection. A call that
/// names a namespace with no live stream has no queue, so it is refused rather than
/// run — and the refusal names that namespace, because "that stream is over" and
/// "you never opened this" are different answers a client acts on differently.
///
/// Dispatching is one non-blocking send onto the namespace's bounded queue, so the
/// read loop never waits for the call: the queue's own task writes the reply (§7.1).
/// A call that does not fit is refused rather than queued or dropped — waiting for
/// room here would hold the `close` this loop alone can answer (§5, §7.4).
fn call(id: StreamId, op: String, input: Value, meta: &Value, conn: &Conn<'_>) {
    let Conn { registry, out, .. } = *conn;
    let Some(ns) = Frame::op_namespace(&op) else {
        // A name with no namespace cannot address any stream, which the client's own
        // check calls `InvalidInput`; the server must not answer it differently.
        let why = format!("`{op}` is not a namespaced op");
        fail(out, id, ErrorKind::InvalidInput, &why);
        return;
    };
    let ns = ns.to_string();
    let sender = locked(registry).sender_for(&ns, conn);
    let Some(sender) = sender else {
        fail(
            out,
            id,
            ErrorKind::Forbidden,
            &format!(
                "`{op}` is outside every namespace this connection opened: \
                 no stream in `{ns}` is live"
            ),
        );
        return;
    };
    let queued = sender.try_send(Call {
        op,
        input,
        ctx: Ctx::from_meta(meta),
        id,
    });
    match queued {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => fail(
            out,
            id,
            ErrorKind::Internal,
            &format!(
                "the `{ns}` call queue is full ({CALL_QUEUE} calls already waiting), \
                 so this one was not queued"
            ),
        ),
        // The queue's task is gone only when the connection is going with it, and a
        // client that never hears back cannot tell that from a slow op.
        Err(mpsc::error::TrySendError::Closed(_)) => fail(
            out,
            id,
            ErrorKind::Internal,
            &format!("the `{ns}` call queue is closed: the connection is going away"),
        ),
    }
}
