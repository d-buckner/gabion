//! The browser stream transport: one `WebSocket` per channel, written as frames
//! (`docs/streaming.md` §5).
//!
//! This is the case the wasm `Send` split exists for. A `web_sys::WebSocket` is
//! neither `Send` nor `Sync`, so it cannot sit behind a `Send + Sync` box the way a
//! `tokio_tungstenite` socket can. It lives in the driver, and the driver is boxed
//! without those bounds on wasm — which is what [`gabion::channel::Channel`]
//! already says about a single-threaded host.
//!
//! The driver is therefore wasm-only. This crate still builds for a host target —
//! `cargo clippy --workspace` builds everything — and there the capability answers
//! that it is not a browser instead of pretending to hold a socket.

use gabion::caps::{HttpRequest, StreamError, Streams};
#[cfg(not(target_arch = "wasm32"))]
use gabion::channel::Channel;
#[cfg(target_arch = "wasm32")]
use gabion::channel::{Channel, ChannelDriver, SendFrame};
#[cfg(target_arch = "wasm32")]
use gabion::frames::Frame;
#[cfg(target_arch = "wasm32")]
use gabion::stream::{BoxStream, Stream};
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::collections::VecDeque;
#[cfg(target_arch = "wasm32")]
use std::pin::Pin;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_arch = "wasm32")]
use std::task::{Context, Poll, Waker};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

/// The wasm `Streams`: a browser `WebSocket` to `request.url`.
///
/// The capability itself holds nothing, so it is `Send + Sync` like every other
/// capability. The socket lives in the channel it opens.
#[derive(Clone, Copy, Default)]
pub struct WebStreams;

impl WebStreams {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// In a browser, a `WebSocket` to `request.url`.
#[cfg(target_arch = "wasm32")]
impl Streams for WebStreams {
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError> {
        let socket = WebSocket::new(&request.url)
            .map_err(|_| StreamError::Transport(format!("{} refused a connection", request.url)))?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let read = Rc::new(ReadSide::default());
        // The closures borrow nothing from the driver, so they can own the state
        // they write. Dropped closures stop the socket from reporting anything.
        let on_message = {
            let read = Rc::clone(&read);
            Closure::wrap(Box::new(move |event: MessageEvent| {
                read.push(match event.data().as_string() {
                    Some(text) => decode(&text),
                    None => {
                        let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
                        match String::from_utf8(bytes) {
                            Ok(text) => decode(&text),
                            Err(_) => {
                                Err(StreamError::Protocol("a frame that is not UTF-8".into()))
                            }
                        }
                    }
                })
            }) as Box<dyn FnMut(MessageEvent)>)
        };
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let on_error = {
            let read = Rc::clone(&read);
            Closure::wrap(Box::new(move |event: ErrorEvent| {
                read.fail(StreamError::Transport(format!(
                    "the socket reported an error: {}",
                    event.message()
                )));
            }) as Box<dyn FnMut(ErrorEvent)>)
        };
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let on_close = {
            let read = Rc::clone(&read);
            Closure::wrap(Box::new(move |event: CloseEvent| {
                if event.was_clean() {
                    read.finish();
                } else {
                    read.fail(StreamError::Transport(format!(
                        "the socket closed uncleanly with code {}",
                        event.code()
                    )));
                }
            }) as Box<dyn FnMut(CloseEvent)>)
        };
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        // A browser throws on `send` until the socket finishes connecting, and
        // gabion writes a channel's first frame the moment the channel opens.
        // Frames written before `open` wait here and go out on the `open` event.
        let pending: Rc<RefCell<VecDeque<String>>> = Rc::new(RefCell::new(VecDeque::new()));
        let on_open = {
            let read = Rc::clone(&read);
            let pending = Rc::clone(&pending);
            let socket = socket.clone();
            Closure::wrap(Box::new(move || {
                while let Some(text) = pending.borrow_mut().pop_front() {
                    if socket.send_with_str(&text).is_err() {
                        read.fail(StreamError::Transport(
                            "the socket refused a frame queued before it opened".into(),
                        ));
                        break;
                    }
                }
            }) as Box<dyn FnMut()>)
        };
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        Ok(Channel::new(WsDriver {
            socket,
            read,
            pending,
            closed: AtomicBool::new(false),
            _handlers: (on_message, on_error, on_close, on_open),
        }))
    }
}

#[cfg(target_arch = "wasm32")]
fn decode(text: &str) -> Result<Frame, StreamError> {
    gabion::serde_json::from_str(text)
        .map_err(|e| StreamError::Protocol(format!("frame would not parse: {e}")))
}

/// Anywhere else, there is no browser socket to open.
#[cfg(not(target_arch = "wasm32"))]
impl Streams for WebStreams {
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError> {
        Err(StreamError::Transport(format!(
            "{} is a browser WebSocket and this build is not a browser",
            request.url
        )))
    }
}

/// Frames waiting to be read, plus whoever is waiting for them.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct ReadSide {
    waiting: RefCell<Waiters>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct Waiters {
    frames: VecDeque<Result<Frame, StreamError>>,
    finished: bool,
    waker: Option<Waker>,
}

#[cfg(target_arch = "wasm32")]
impl ReadSide {
    fn push(&self, frame: Result<Frame, StreamError>) {
        let mut waiting = self.waiting.borrow_mut();
        waiting.frames.push_back(frame);
        if let Some(waker) = waiting.waker.take() {
            waker.wake();
        }
    }

    /// The socket is gone and said so. The codec still has to decide whether an
    /// `end` frame arrived (§7.4), so this stops the read side and says nothing
    /// about why.
    fn finish(&self) {
        let mut waiting = self.waiting.borrow_mut();
        waiting.finished = true;
        if let Some(waker) = waiting.waker.take() {
            waker.wake();
        }
    }

    fn fail(&self, error: StreamError) {
        self.push(Err(error));
        self.finish();
    }
}

#[cfg(target_arch = "wasm32")]
type Handlers = (
    Closure<dyn FnMut(MessageEvent)>,
    Closure<dyn FnMut(ErrorEvent)>,
    Closure<dyn FnMut(CloseEvent)>,
    Closure<dyn FnMut()>,
);

#[cfg(target_arch = "wasm32")]
struct WsDriver {
    socket: WebSocket,
    read: Rc<ReadSide>,
    /// Frames written while the socket was still connecting, in order.
    pending: Rc<RefCell<VecDeque<String>>>,
    /// Set once `close` runs, so a late `onclose` does not overwrite the reason.
    closed: AtomicBool,
    /// Kept alive for the socket's lifetime; the browser calls them.
    _handlers: Handlers,
}

#[cfg(target_arch = "wasm32")]
impl ChannelDriver for WsDriver {
    fn send(&self, frame: Frame) -> SendFrame<'_> {
        let outcome = gabion::serde_json::to_string(&frame)
            .map_err(|e| StreamError::Protocol(format!("frame would not serialize: {e}")))
            .and_then(|text| self.write(text));
        Box::pin(async { outcome })
    }

    fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>> {
        Box::pin(ReadQueue {
            read: Rc::clone(&self.read),
        })
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // A socket still connecting throws on close. There is nothing to recover:
        // the caller asked for the channel to stop, and it stops either way.
        let _ = self.socket.close();
        self.pending.borrow_mut().clear();
        self.read.finish();
    }
}

#[cfg(target_arch = "wasm32")]
impl WsDriver {
    /// Send one frame now, or hold it until the socket opens. A socket that is
    /// closing or closed throws on send, so the browser answers that question
    /// instead of gabion guessing it from a state enum.
    fn write(&self, text: String) -> Result<(), StreamError> {
        match self.socket.ready_state() {
            WebSocket::CONNECTING => {
                self.pending.borrow_mut().push_back(text);
                Ok(())
            }
            WebSocket::OPEN => self
                .socket
                .send_with_str(&text)
                .map_err(|_| StreamError::Transport("the socket would not take the frame".into())),
            state => Err(StreamError::Transport(format!(
                "the socket is not open (ready state {state})"
            ))),
        }
    }
}

/// The read side of a browser socket: whatever the callbacks queued.
#[cfg(target_arch = "wasm32")]
struct ReadQueue {
    read: Rc<ReadSide>,
}

#[cfg(target_arch = "wasm32")]
impl Stream for ReadQueue {
    type Item = Result<Frame, StreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let waiting = &mut *self.read.waiting.borrow_mut();
        if let Some(frame) = waiting.frames.pop_front() {
            return Poll::Ready(Some(frame));
        }
        if waiting.finished {
            return Poll::Ready(None);
        }
        waiting.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
