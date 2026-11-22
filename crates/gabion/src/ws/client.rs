//! The client half of the stream surface: one connection, multiplexed by stream
//! id, with no reader task.
//!
//! gabion cannot spawn, and a wasm host has no thread to spawn on, so whoever needs
//! a frame asks for one and the frame is routed to whoever it belongs to. That is
//! why a stalled `Each` subscriber stalls the connection instead of a queue: the
//! documented limit of §5, made visible rather than buffered away.

use crate::caps::{Channel, HttpRequest, StreamError, Streams};
use crate::channel::Items;
use crate::ctx::Ctx;
use crate::error::{ErrorKind, FromWire, WireError};
use crate::frames::{Frame, StreamId};
use crate::stream::{next, BoxStream, EndReason, Stream};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context as PollContext, Poll, Waker};

/// One connection to one host, shared by every stream and every `call` of the
/// features that opened them.
///
/// Cloning it hands the same socket to another caller, which is what a client
/// needs: one `/api/ws` per host, not one per feature.
#[derive(Clone)]
pub struct Connection<S: Streams> {
    streams: S,
    url: String,
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    channel: Option<Channel>,
    frames: Option<BoxStream<'static, Result<Frame, StreamError>>>,
    /// Frames that arrived for a stream nobody has taken yet.
    queues: HashMap<StreamId, VecDeque<Frame>>,
    /// One consumer per stream id, so a frame wakes its own reader and nobody
    /// else's.
    waiting: HashMap<StreamId, Waker>,
    /// The failure that ended the read side, once there was one.
    dead: Option<WireError>,
    /// Which namespace each live stream belongs to, and how many are live in each.
    /// The server's wall lets a `call` through only in a namespace with a stream
    /// open, so the client has to know the same thing to choose a surface (§5).
    live: HashMap<StreamId, String>,
    per_ns: HashMap<String, usize>,
    next_id: u64,
}

/// Take the shared state. A panic in one caller's poll left the queues intact, so
/// there is nothing to reconstruct and failing every later caller would turn one
/// panic into a connection-wide mystery.
fn lock(inner: &Arc<Mutex<Inner>>) -> MutexGuard<'_, Inner> {
    match inner.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl<S: Streams> Connection<S> {
    #[must_use]
    pub fn new(url: impl Into<String>, streams: S) -> Self {
        Self {
            streams,
            url: url.into(),
            inner: Arc::new(Mutex::new(Inner {
                channel: None,
                frames: None,
                queues: HashMap::new(),
                waiting: HashMap::new(),
                dead: None,
                live: HashMap::new(),
                per_ns: HashMap::new(),
                next_id: 1,
            })),
        }
    }

    /// The endpoint this connection dials.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Open the channel on first use. Never held across an await: `Streams::open`
    /// is synchronous for exactly this reason.
    fn ensure(&self) -> Result<Channel, WireError> {
        let mut inner = lock(&self.inner);
        if let Some(channel) = &inner.channel {
            return Ok(channel.clone());
        }
        let channel = self
            .streams
            .open(HttpRequest::post(self.url.clone(), Vec::new()))
            .map_err(|e| WireError::new(ErrorKind::Internal, e.to_string()))?;
        inner.frames = Some(channel.incoming());
        inner.channel = Some(channel.clone());
        Ok(channel)
    }

    /// Run one ordinary op of this feature on the connection.
    pub async fn call(&self, op: &str, input: Value, ctx: &Ctx) -> Result<Value, WireError> {
        let id = self.reserve();
        let channel = self.ensure()?;
        channel
            .send(Frame::call(id, op, input, ctx))
            .await
            .map_err(|e| e.to_wire())?;
        let mut receiver = Receiver {
            inner: Arc::clone(&self.inner),
            id,
        };
        match next(&mut receiver).await {
            Some(Ok(Frame::Reply { ok, .. })) => Ok(ok),
            // The refusal of a call arrives as an `error` frame naming the same
            // stream id, so it is the answer. Turning it into "not a reply" would
            // hide the one thing the caller needs: why the call did not run.
            Some(Ok(Frame::Error { error, .. })) => Err(error),
            Some(Ok(other)) => Err(WireError::new(
                ErrorKind::Internal,
                format!("{other:?} is not a reply to a call"),
            )),
            Some(Err(e)) => Err(e.to_wire()),
            None => Err(WireError::new(
                ErrorKind::Internal,
                "the connection ended before the reply",
            )),
        }
    }

    /// Start a stream and decode its items, the way a generated client does it.
    ///
    /// The namespace comes from the op that was opened, so the codec can refuse an
    /// `item` frame that names a different feature (§5). A caller that took the
    /// raw [`Receiver`] instead would see frames it has no type for.
    ///
    /// The first frame is taken here rather than left to the stream, because a
    /// refusal arrives as an `error` frame and a `Result` is the only place a
    /// caller can read one. An item is put back and comes out of the stream
    /// untouched, so nothing is consumed that the caller would have seen.
    pub async fn stream<T, E>(
        &self,
        op: &str,
        input: Value,
        ctx: &Ctx,
    ) -> Result<BoxStream<'static, Result<T, E>>, WireError>
    where
        T: DeserializeOwned + 'static,
        E: FromWire + 'static,
    {
        let ns = Frame::op_namespace(op)
            .ok_or_else(|| {
                WireError::new(ErrorKind::InvalidInput, format!("`{op}` is not namespaced"))
            })?
            .to_string();
        let receiver = self.open(op, input, ctx).await?;
        let id = receiver.id();
        let mut peeking = Receiver {
            inner: Arc::clone(&self.inner),
            id,
        };
        let first = match next(&mut peeking).await {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => return Err(e.to_wire()),
            None => {
                return Err(WireError::new(
                    ErrorKind::Internal,
                    "the connection ended before the first frame",
                ))
            }
        };
        if let Frame::Error { error, .. } = first {
            return Err(error);
        }
        lock(&self.inner)
            .queues
            .entry(id)
            .or_default()
            .push_front(first);
        Ok(Box::pin(Items::new(id, ns, Box::pin(receiver))))
    }

    /// Start a stream and hand back the frames that belong to it.
    pub async fn open(&self, op: &str, input: Value, ctx: &Ctx) -> Result<Receiver, WireError> {
        let id = self.reserve();
        let ns = Frame::op_namespace(op)
            .ok_or_else(|| {
                WireError::new(ErrorKind::InvalidInput, format!("`{op}` is not namespaced"))
            })?
            .to_string();
        let channel = self.ensure()?;
        channel
            .send(Frame::open(id, op, input, ctx))
            .await
            .map_err(|e| e.to_wire())?;
        {
            let mut inner = lock(&self.inner);
            inner.live.insert(id, ns.clone());
            *inner.per_ns.entry(ns).or_insert(0) += 1;
        }
        Ok(Receiver {
            inner: Arc::clone(&self.inner),
            id,
        })
    }

    /// Whether a stream in namespace `ns` is live on this connection.
    ///
    /// This is what decides a streaming feature's surface for an ordinary op. The
    /// server's wall lets a `call` frame through only in a namespace the client
    /// already opened a stream in, so before the first stream the op must still be a
    /// POST, and after one it must not be — §5's "a feature that streams puts all of
    /// its ops on the connection" is conditional on the connection having one.
    #[must_use]
    pub fn has_open_stream(&self, ns: &str) -> bool {
        lock(&self.inner).per_ns.get(ns).copied().unwrap_or(0) > 0
    }

    /// Stop a stream this connection opened, and report how it ended.
    ///
    /// The server answers a `close` it cannot honour with an `error` frame, so the
    /// outcome is read back rather than assumed. A client that fired the frame and
    /// moved on could not tell "stopped" from "you never opened that".
    pub async fn close(&self, id: StreamId) -> Result<EndReason, WireError> {
        let channel = self.ensure()?;
        channel
            .send(Frame::Close { id })
            .await
            .map_err(|e| e.to_wire())?;
        let mut receiver = Receiver {
            inner: Arc::clone(&self.inner),
            id,
        };
        // Items already in flight reach the reader before the `end` the close
        // causes, so the close's own answer is the first terminator, not the
        // first frame.
        loop {
            match next(&mut receiver).await {
                Some(Ok(Frame::End { reason, .. })) => return Ok(reason),
                Some(Ok(Frame::Error { error, .. })) => return Err(error),
                Some(Ok(Frame::Item { .. })) => continue,
                Some(Ok(other)) => {
                    return Err(WireError::new(
                        ErrorKind::Internal,
                        format!("{other:?} is not an answer to a close"),
                    ))
                }
                Some(Err(e)) => return Err(e.to_wire()),
                None => {
                    return Err(WireError::new(
                        ErrorKind::Internal,
                        "the connection ended before the close was answered",
                    ))
                }
            }
        }
    }

    fn reserve(&self) -> StreamId {
        let mut inner = lock(&self.inner);
        // From 1: `StreamId(0)` is the no-stream id an unparseable frame is
        // answered under, so no stream this client opens may claim it.
        let id = StreamId(inner.next_id);
        inner.next_id += 1;
        id
    }
}

/// The frames of one stream, taken from the shared connection.
///
/// Dropping it stops delivery; the server is told by the surface that opened it,
/// not here, because a `close` frame needs the channel and this type holds only a
/// route to the read side.
pub struct Receiver {
    inner: Arc<Mutex<Inner>>,
    id: StreamId,
}

impl Receiver {
    /// The stream id this receiver reads.
    #[must_use]
    pub fn id(&self) -> StreamId {
        self.id
    }
}

/// Hand the shared read side over to whoever is left.
///
/// The read side wakes only the task that polled it last, and gabion cannot keep a
/// reader of its own. So a reader that goes away while another is parked would
/// leave that one parked with a frame already on the wire: nothing would ever poll
/// the socket again. Waking every parked reader makes one of them pick the read
/// side back up and route what is waiting.
impl Drop for Receiver {
    fn drop(&mut self) {
        let waiting: Vec<Waker> = lock(&self.inner).waiting.drain().map(|(_, w)| w).collect();
        for waker in waiting {
            waker.wake();
        }
    }
}

impl Stream for Receiver {
    type Item = Result<Frame, StreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let mut inner = lock(&this.inner);
        loop {
            if let Some(frame) = inner.queues.get_mut(&this.id).and_then(VecDeque::pop_front) {
                return Poll::Ready(Some(Ok(frame)));
            }
            if let Some(error) = &inner.dead {
                return Poll::Ready(Some(Err(StreamError::Transport(error.message.clone()))));
            }
            match route(&mut inner, cx) {
                Read::Pending => {
                    inner.waiting.insert(this.id, cx.waker().clone());
                    return Poll::Pending;
                }
                Read::Dead(why) => {
                    inner.dead = Some(WireError::new(ErrorKind::Internal, why));
                }
                // The frame went to its own queue. Loop, so this reader takes its
                // own if that was the one that landed.
                Read::Taken => continue,
            }
        }
    }
}

/// One attempt to move a frame off the shared source.
enum Read {
    /// Nothing was available; the caller should wait.
    Pending,
    /// A frame was read and queued under the id it names.
    Taken,
    /// The read side is finished, and why.
    Dead(String),
}

/// Read one frame off the shared source and queue it under the id it names.
///
/// Routing lives here rather than in each reader so a frame can only ever reach the
/// stream it names (§5): one place decides, and it never looks at who asked.
fn route(inner: &mut Inner, cx: &mut PollContext<'_>) -> Read {
    let Some(frames) = inner.frames.as_mut() else {
        return Read::Dead("the connection has no read side".into());
    };
    match frames.as_mut().poll_next(cx) {
        Poll::Pending => Read::Pending,
        Poll::Ready(None) => Read::Dead("the connection closed without an end".into()),
        Poll::Ready(Some(Err(e))) => Read::Dead(e.to_string()),
        Poll::Ready(Some(Ok(frame))) => {
            let target = frame.id();
            if matches!(frame, Frame::End { .. } | Frame::Error { .. }) {
                if let Some(ns) = inner.live.remove(&target) {
                    if let Some(count) = inner.per_ns.get_mut(&ns) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
            inner.queues.entry(target).or_default().push_back(frame);
            if let Some(waker) = inner.waiting.remove(&target) {
                waker.wake();
            }
            Read::Taken
        }
    }
}
