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
//!   One comparison against the open-stream registry, at the one place that has it.
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

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use futures_util::{SinkExt, StreamExt};
use gabion::ctx::{Cancel, Ctx};
use gabion::frames::{Frame, StreamId};
use gabion::ws::WsService;

use pump::{fail, pump};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Take the open-stream registry. A panic in one connection's pump leaves the map
/// intact, so there is nothing to reconstruct and dropping the connection would
/// turn one panic into every later caller's mystery.
fn locked(registry: &Arc<Mutex<Registry>>) -> std::sync::MutexGuard<'_, Registry> {
    match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

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
struct Surface {
    services: Vec<Arc<dyn WsService>>,
}

impl Surface {
    fn find(&self, op: &str) -> Option<&Arc<dyn WsService>> {
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
                        fail(&out, StreamId(0), &format!("frame would not parse: {e}")).await;
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
    cancel_all(&registry).await;
    drop(out);
    let _ = writer.await;
}

/// Streams this connection opened. The one record the namespace wall reads.
#[derive(Default)]
struct Registry {
    open: HashMap<StreamId, Open>,
    /// One ordered queue per namespace, so calls in a namespace keep arrival order.
    namespaces: HashMap<String, mpsc::UnboundedSender<Call>>,
}

struct Open {
    ns: String,
    cancel: Cancel,
}

struct Call {
    op: String,
    input: Value,
    ctx: Ctx,
    id: StreamId,
}

/// Everything one frame handler needs about the connection it came in on.
struct Conn<'a> {
    surface: &'a Arc<Surface>,
    registry: &'a Arc<Mutex<Registry>>,
    out: &'a mpsc::Sender<Frame>,
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
            let gone = locked(registry).open.remove(&id);
            match gone {
                Some(entry) => {
                    entry.cancel.cancel();
                }
                None => {
                    fail(
                        out,
                        id,
                        &format!("stream {id} was not opened on this connection"),
                    )
                    .await;
                }
            }
            true
        }
        other => {
            fail(
                out,
                other.id(),
                &format!("{other:?} is not a frame a client sends"),
            )
            .await;
            true
        }
    }
}

impl Registry {
    /// The ordered queue for a namespace, only if a stream in it is already open.
    ///
    /// A queue lives for the connection, not for the streams in it. Retiring one
    /// when its last stream closed would drop the calls already queued behind it:
    /// a `close` ends one stream, and §7.1 promises the others their turn.
    fn sender_for(&mut self, ns: &str, conn: &Conn<'_>) -> Option<mpsc::UnboundedSender<Call>> {
        if !self.open.values().any(|o| o.ns == ns) {
            return None;
        }
        if let Some(sender) = self.namespaces.get(ns) {
            return Some(sender.clone());
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        self.namespaces.insert(ns.to_string(), sender.clone());
        tokio::spawn(drain_calls(
            receiver,
            Arc::clone(conn.surface),
            conn.out.clone(),
        ));
        Some(sender)
    }
}

/// Calls for one namespace, in the order they arrived (§7.1).
///
/// Sequential within the namespace, and one task per namespace, so a slow op in one
/// feature does not hold another's. Writing the reply is awaited here rather than
/// spawned, or the second reply could reach the socket before the first.
async fn drain_calls(
    mut receiver: mpsc::UnboundedReceiver<Call>,
    surface: Arc<Surface>,
    out: mpsc::Sender<Frame>,
) {
    while let Some(call) = receiver.recv().await {
        let frame = match surface.find(&call.op) {
            Some(service) => match service.call(&call.ctx, &call.op, call.input).await {
                Ok(ok) => Frame::Reply { id: call.id, ok },
                Err(error) => Frame::Error { id: call.id, error },
            },
            None => Frame::Error {
                id: call.id,
                error: gabion::error::WireError::new(
                    gabion::error::ErrorKind::NotFound,
                    format!("no op named `{}`", call.op),
                ),
            },
        };
        if out.send(frame).await.is_err() {
            return;
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
        fail(out, id, &format!("no op named `{op}`")).await;
        return;
    };
    let ns = service.ns().to_string();
    let cancel = Cancel::new();
    locked(registry).open.insert(
        id,
        Open {
            ns: ns.clone(),
            cancel: cancel.clone(),
        },
    );
    let ctx = ctx.with_cancel(cancel);
    let started = service.open(&ctx, &op, input).await;
    let opened = match started {
        Ok(opened) => opened,
        Err(error) => {
            let _ = out.send(Frame::Error { id, error }).await;
            locked(registry).open.remove(&id);
            return;
        }
    };
    tokio::spawn(pump(id, ns, opened, ctx, out.clone()));
}

async fn cancel_all(registry: &Arc<Mutex<Registry>>) {
    let open: Vec<Cancel> = locked(registry)
        .open
        .values()
        .map(|o| o.cancel.clone())
        .collect();
    for cancel in open {
        cancel.cancel();
    }
    locked(registry).open.clear();
}

/// One ordinary op on a stream this connection already opened (§5).
///
/// The namespace wall is one lookup here and nowhere else: the queue a call joins
/// is the queue of the namespace that opened a stream on this connection. A call
/// that names a namespace nobody opened has no queue, so it is refused rather than
/// run — which is the whole of the isolation the design promises.
async fn call(id: StreamId, op: String, input: Value, meta: &Value, conn: &Conn<'_>) {
    let Conn { registry, out, .. } = *conn;
    let Some(ns) = Frame::op_namespace(&op) else {
        fail(out, id, &format!("`{op}` is not a namespaced op")).await;
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
                &format!("`{op}` is outside every namespace this connection opened"),
            )
            .await;
        }
    }
}
