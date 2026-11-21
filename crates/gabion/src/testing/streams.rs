//! Scripted streams for tests (S14).
//!
//! A test says what one channel will deliver and reads back what was written to
//! it. The three shapes a stream test needs are all here: a fixed item list, a
//! scripted failure mid-stream, and a scripted stall. Without them, stream
//! conformance is nondeterministic by construction — which is the hole S14 exists
//! to close.
//!
//! The channel waits for the client's `open` frame before it produces anything,
//! so a test exercises the same frame sequence a real server sees.

use crate::caps::{Channel, HttpRequest, StreamError, Streams};
use crate::channel::{ChannelDriver, SendFrame};
use crate::error::WireError;
use crate::frames::{Frame, StreamId};
use crate::stream::{BoxStream, EndReason, Stream};
use serde_json::Value;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

/// What arrives after the scripted items.
#[derive(Clone)]
pub enum Tail {
    /// An `end done` frame, then nothing. The clean end.
    Done,
    /// An `error` frame carrying `error`, then nothing.
    Failed(WireError),
    /// Nothing more ever arrives, and nothing says why. A deadline or a close is
    /// the only way out, which is what makes this the stall case.
    Stalled,
    /// The channel dies with no `end` and no `error` frame at all. §7.4 says this
    /// must never read as a completed stream.
    Dead,
}

/// One channel's script.
#[derive(Clone)]
pub struct Script {
    items: Vec<Value>,
    tail: Tail,
}

impl Script {
    /// A fixed item list, then a clean end.
    #[must_use]
    pub fn items(items: Vec<Value>) -> Self {
        Self {
            items,
            tail: Tail::Done,
        }
    }

    /// The items, then a typed failure mid-stream.
    #[must_use]
    pub fn failing(items: Vec<Value>, error: WireError) -> Self {
        Self {
            items,
            tail: Tail::Failed(error),
        }
    }

    /// The items, then silence forever.
    #[must_use]
    pub fn stalling(items: Vec<Value>) -> Self {
        Self {
            items,
            tail: Tail::Stalled,
        }
    }

    /// The items, then a channel that dies without stating an end.
    #[must_use]
    pub fn dying(items: Vec<Value>) -> Self {
        Self {
            items,
            tail: Tail::Dead,
        }
    }
}

/// Scripted streams. A channel nobody scripted is an error, not an empty stream.
#[derive(Clone, Default)]
pub struct MockStreams {
    scripts: Arc<Mutex<BTreeMap<String, Script>>>,
    refused: Arc<Mutex<BTreeMap<String, String>>>,
    opened: Arc<Mutex<Vec<HttpRequest>>>,
    written: Arc<Mutex<Vec<Frame>>>,
}

impl MockStreams {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the channel at `url`.
    #[must_use]
    pub fn script(self, url: &str, script: Script) -> Self {
        self.lock(&self.scripts).insert(url.to_string(), script);
        self
    }

    /// The channel at `url` cannot be opened at all.
    #[must_use]
    pub fn refuse(self, url: &str, why: &str) -> Self {
        self.lock(&self.refused)
            .insert(url.to_string(), why.to_string());
        self
    }

    /// Every `HttpRequest` this capability was asked to open, in order.
    #[must_use]
    pub fn opened(&self) -> Vec<HttpRequest> {
        self.lock(&self.opened).clone()
    }

    /// Every frame written to a channel this capability handed out, in order.
    #[must_use]
    pub fn written(&self) -> Vec<Frame> {
        self.lock(&self.written).clone()
    }

    fn lock<'a, T>(&self, m: &'a Arc<Mutex<T>>) -> MutexGuard<'a, T> {
        m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Streams for MockStreams {
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError> {
        self.lock(&self.opened).push(request.clone());
        if let Some(why) = self.lock(&self.refused).get(&request.url).cloned() {
            return Err(StreamError::Transport(why));
        }
        let script = self
            .lock(&self.scripts)
            .get(&request.url)
            .cloned()
            .ok_or_else(|| {
                StreamError::Transport(format!("no scripted stream for {}", request.url))
            })?;
        Ok(Channel::new(MockChannel {
            written: Arc::clone(&self.written),
            state: Arc::new(Mutex::new(ChannelState {
                script,
                opened: None,
                next_item: 0,
                finished: false,
                closed: false,
                waker: None,
            })),
        }))
    }
}

/// The state one mock channel shares between its two halves, so a `close` can
/// wake a read parked on a stall.
struct ChannelState {
    script: Script,
    /// The stream the client opened, as `(id, namespace)`. The namespace comes
    /// from the op name, the way a real server derives it.
    opened: Option<(StreamId, String)>,
    next_item: usize,
    finished: bool,
    closed: bool,
    waker: Option<Waker>,
}

struct MockChannel {
    state: Arc<Mutex<ChannelState>>,
    written: Arc<Mutex<Vec<Frame>>>,
}

impl MockChannel {
    fn lock(&self) -> MutexGuard<'_, ChannelState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wakes a parked read, if one is parked.
    fn wake(&self, state: &mut ChannelState) {
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl ChannelDriver for MockChannel {
    fn send(&self, frame: Frame) -> SendFrame<'_> {
        {
            let mut state = self.lock();
            match frame {
                Frame::Open { id, ref op, .. } => {
                    let ns = Frame::op_namespace(op).unwrap_or("").to_string();
                    state.opened = Some((id, ns));
                }
                Frame::Close { .. } => state.closed = true,
                _ => {}
            }
            self.wake(&mut state);
            if let Ok(mut written) = self.written.lock() {
                written.push(frame);
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>> {
        Box::pin(MockIncoming {
            state: Arc::clone(&self.state),
        })
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        self.wake(&mut state);
    }
}

/// The read half of a [`MockChannel`].
struct MockIncoming {
    state: Arc<Mutex<ChannelState>>,
}

impl Stream for MockIncoming {
    type Item = Result<Frame, StreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((id, ns)) = state.opened.clone() else {
            // Nothing until the client says which stream to feed.
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        };
        if state.closed || state.finished {
            return Poll::Ready(None);
        }
        if state.next_item < state.script.items.len() {
            let data = state.script.items[state.next_item].clone();
            state.next_item += 1;
            return Poll::Ready(Some(Ok(Frame::Item { id, ns, data })));
        }
        match state.script.tail.clone() {
            Tail::Done => {
                state.finished = true;
                Poll::Ready(Some(Ok(Frame::End {
                    id,
                    reason: EndReason::Done,
                })))
            }
            Tail::Failed(error) => {
                state.finished = true;
                Poll::Ready(Some(Ok(Frame::Error { id, error })))
            }
            // A dead channel stops without saying so. The codec, not the mock,
            // decides what that means (§7.4).
            Tail::Dead => Poll::Ready(None),
            Tail::Stalled => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
