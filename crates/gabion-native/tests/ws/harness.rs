//! The socket harness both halves of the stream suite share: two features, a real
//! listener, a real client `Connection`, and the two producers a flow-policy test
//! needs — one that ends, and one that only stops when its cancel token does.
//!
//! Split out because the two suites are about different rules and each reads better
//! on its own; the wall tests do not need a ticking producer and the flow tests do
//! not need a second feature's namespace.
#![allow(clippy::unwrap_used, clippy::expect_used)]
// This module is a shared prelude on purpose: each suite takes what it needs through
// `use harness::*`, so an item only one suite touches looks unused from in here.
#![allow(unused_imports, dead_code)]

pub use gabion::ctx::{Cancel, Ctx};
pub use gabion::error::{ErrorKind, WireError};
pub use gabion::frames::{Frame, StreamId};
pub use gabion::stream::{EndReason, Stream};
pub use gabion::ws::{CallResult, Connection, Op, OpenResult, Opened, Policy, Receiver, WsService};
pub use gabion_native::WsStreams;
pub use serde_json::{json, Value};
pub use std::future::Future;
pub use std::pin::Pin;
pub use std::sync::atomic::{AtomicUsize, Ordering};
pub use std::sync::{Arc, Mutex};
pub use std::task::{Context, Poll};
pub use std::time::Duration;
pub use tokio::time::Instant;

// The raw-socket helpers below use the sink and stream halves the way the transport
// does; nothing else in the suites needs them in scope.
use futures_util::{SinkExt as _, StreamExt as _};

/// How many items a feature's stream gives before it ends on its own.
pub const FOREVER: usize = usize::MAX;

/// One feature: two listen ops and two ordinary ones. Two of these on one host is
/// the shape the namespace wall exists for.
pub struct Feature {
    pub ns: &'static str,
    pub items: usize,
    /// How long the producer waits between items. A producer that is never ready
    /// is never asked for another item, so a close has nothing to interrupt.
    pub every: Duration,
    /// Incremented when the producer is gone. The evidence for
    /// `a_dropped_socket_ends_the_producer_even_while_it_is_idle`.
    ///
    /// Counted on drop rather than where the token is observed, because a producer
    /// has two honest ways to stop: it can poll a cancelled token, or the write it was
    /// blocked on can fail and tear the stream down. Both end the producer; only one
    /// of them polls the token, and the surface promises the first thing.
    pub stopped: Arc<AtomicUsize>,
}

impl Feature {
    pub fn new(ns: &'static str, items: usize) -> Self {
        Self {
            ns,
            items,
            every: Duration::ZERO,
            stopped: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A feature whose stream never ends by itself, ticking every `every`.
    pub fn open_ended(ns: &'static str, every: Duration) -> Self {
        Self {
            items: FOREVER,
            every,
            ..Self::new(ns, FOREVER)
        }
    }
}

impl WsService for Feature {
    fn ns(&self) -> &str {
        self.ns
    }

    fn ops(&self) -> Vec<Op> {
        vec![
            Op {
                name: format!("{}.watch", self.ns),
                policy: Some(Policy::Each),
            },
            Op {
                name: format!("{}.tail", self.ns),
                policy: Some(Policy::Latest),
            },
            Op {
                name: format!("{}.ping", self.ns),
                policy: None,
            },
            Op {
                name: format!("{}.slow", self.ns),
                policy: None,
            },
        ]
    }

    fn call<'a>(&'a self, ctx: &'a Ctx, op: &'a str, input: Value) -> CallResult<'a> {
        let ns = self.ns;
        let delay = input.get("ms").and_then(Value::as_u64).unwrap_or(0);
        let principal = ctx.principal().unwrap_or_default().to_string();
        Box::pin(async move {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if op == format!("{ns}.ping") || op == format!("{ns}.slow") {
                return Ok(json!({ "op": op, "delay": delay, "who": principal }));
            }
            Err(WireError::new(
                ErrorKind::NotFound,
                format!("no op named `{op}` on {ns}"),
            ))
        })
    }

    fn open<'a>(&'a self, ctx: &'a Ctx, op: &'a str, _input: Value) -> OpenResult<'a> {
        let count = self.items;
        let every = self.every;
        let ns = self.ns;
        let policy = if op.ends_with(".tail") {
            Policy::Latest
        } else {
            Policy::Each
        };
        let cancel = ctx.cancel_token().clone();
        let stopped = Arc::clone(&self.stopped);
        Box::pin(async move {
            if !op.starts_with(&format!("{ns}.")) {
                return Err(WireError::new(
                    ErrorKind::NotFound,
                    format!("`{op}` is not a listen op on {ns}"),
                ));
            }
            Ok(Opened {
                policy,
                items: Box::pin(Ticking {
                    left: count,
                    seen: 0,
                    every,
                    until: None,
                    cancel,
                    stopped,
                }),
            })
        })
    }
}

/// One item per `every`, ending after `left` of them, watching a cancel token.
///
/// The wait is the point. A producer that is always ready is never parked, so it
/// never has anything for a close or a disconnect to interrupt — and §7.4 is about
/// exactly the producer that is sitting idle when the socket goes.
pub struct Ticking {
    pub left: usize,
    seen: u64,
    pub every: Duration,
    until: Option<Pin<Box<tokio::time::Sleep>>>,
    pub cancel: Cancel,
    stopped: Arc<AtomicUsize>,
}

impl Drop for Ticking {
    fn drop(&mut self) {
        self.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

impl Stream for Ticking {
    type Item = Result<Value, WireError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.cancel.is_cancelled() {
            return Poll::Ready(None);
        }
        if this.left == 0 {
            return Poll::Ready(None);
        }
        if !this.every.is_zero() {
            let slot = this
                .until
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(this.every)));
            if slot.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            // Re-armed for the next tick, so the stream keeps producing rather than
            // ending after one wait.
            *slot = Box::pin(tokio::time::sleep(this.every));
        }
        this.left -= 1;
        this.seen += 1;
        Poll::Ready(Some(Ok(json!({ "n": this.seen }))))
    }
}

/// Serve one host with two streaming features and hand back its base URL.
///
/// Both default to a short finite stream. A test that needs a stream still running
/// when it acts builds one with `Feature::open_ended` and passes it in.
pub async fn serve() -> (String, Arc<Feature>, Arc<Feature>) {
    serve_with(
        Arc::new(Feature::new("alpha", 3)),
        Arc::new(Feature::new("beta", 2)),
    )
    .await
}

pub async fn serve_with(
    alpha: Arc<Feature>,
    beta: Arc<Feature>,
) -> (String, Arc<Feature>, Arc<Feature>) {
    let services: Vec<Arc<dyn WsService>> = vec![alpha.clone(), beta.clone()];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, gabion_native::ws_router(services))
            .await
            .unwrap();
    });
    (format!("http://{addr}"), alpha, beta)
}

pub fn client(base: &str) -> Connection<WsStreams> {
    Connection::new(format!("{base}/api/ws"), WsStreams)
}

/// A socket this test writes frames on by hand, for the frames `Connection` will
/// not write: a frame that does not parse, or a second `open` under an id it picks
/// for itself.
pub type Raw =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn raw(base: &str) -> Raw {
    let url = format!("{}/api/ws", base.replacen("http://", "ws://", 1));
    let (socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a raw socket to the host");
    socket
}

pub async fn send_raw(socket: &mut Raw, frame: Frame) {
    let text = serde_json::to_string(&frame).expect("serialize");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
        .await
        .expect("the write lands");
}

/// One text frame off the raw socket, skipped past the frames that are not gabion
/// framing. A socket that ends first is a panic: every test here is waiting on a
/// frame the server promised.
pub async fn next_raw(socket: &mut Raw) -> Frame {
    loop {
        match socket.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                return serde_json::from_str(&text).expect("a gabion frame");
            }
            Some(Ok(_)) => continue,
            other => panic!("the socket ended before a frame: {other:?}"),
        }
    }
}

/// Read one stream to its terminator: the items, and how it ended.
pub async fn drain(rx: &mut Receiver) -> (Vec<Value>, String) {
    let mut items = Vec::new();
    while let Some(frame) = gabion::stream::next(rx).await {
        match frame {
            Ok(Frame::Item { data, .. }) => items.push(data),
            Ok(Frame::End { reason, .. }) => {
                return (items, format!("{reason:?}").to_lowercase());
            }
            Ok(Frame::Error { error, .. }) => {
                return (items, format!("error:{}", error.kind.as_str()))
            }
            Ok(other) => return (items, format!("unexpected:{other:?}")),
            Err(e) => return (items, format!("transport:{e}")),
        }
    }
    (items, "no-terminator".to_string())
}
