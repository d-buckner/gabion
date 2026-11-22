//! The client's own multiplexer: one shared read side, many streams, and what a
//! stream that nobody reads is owed (`docs/streaming.md` §5).
//!
//! Driven by a scripted `Streams` that multiplexes two ids on one channel, which
//! `MockStreams` cannot do — that one scripts one stream per channel.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{HttpRequest, StreamError, Streams};
use gabion::channel::{Channel, ChannelDriver, SendFrame};
use gabion::error::ErrorKind;
use gabion::frames::{Frame, StreamId};
use gabion::stream::{BoxStream, Stream};
use gabion::ws::Connection;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

const URL: &str = "ws://test/api/ws";
const OP: &str = "pulse.watch";

/// Resolves a future that is ready in one poll: everything the scripted socket
/// does on a write happens in the call itself.
fn now<T>(mut fut: impl Future<Output = T> + Unpin) -> T {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(&mut fut).poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the future did not resolve in one poll"),
    }
}

fn reading(frame: u32) -> serde_json::Value {
    serde_json::json!({ "frame": frame })
}

/// One channel carrying two streams at once, the way a real server multiplexes
/// them: `total` items for the id that opened first, then one item and an end for
/// the second.
///
/// `MockStreams` scripts one stream per channel, so a test about what one stream
/// does to another's queue cannot use it.
#[derive(Clone)]
struct TwoStreams {
    total: usize,
}

#[derive(Default)]
struct Backlog {
    first: Option<(StreamId, String)>,
    second: Option<(StreamId, String)>,
    sent: usize,
    ended: bool,
}

impl Streams for TwoStreams {
    fn open(&self, _request: HttpRequest) -> Result<Channel, StreamError> {
        Ok(Channel::new(Socket {
            total: self.total,
            backlog: Arc::new(Mutex::new(Backlog::default())),
        }))
    }
}

struct Socket {
    total: usize,
    backlog: Arc<Mutex<Backlog>>,
}

impl Socket {
    fn lock(&self) -> MutexGuard<'_, Backlog> {
        self.backlog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ChannelDriver for Socket {
    fn send(&self, frame: Frame) -> SendFrame<'_> {
        if let Frame::Open { id, ref op, .. } = frame {
            let ns = Frame::op_namespace(op).unwrap_or("").to_string();
            let mut backlog = self.lock();
            if backlog.first.is_none() {
                backlog.first = Some((id, ns));
            } else {
                backlog.second = Some((id, ns));
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>> {
        Box::pin(BacklogStream {
            total: self.total,
            backlog: Arc::clone(&self.backlog),
        })
    }

    fn close(&self) {}
}

struct BacklogStream {
    total: usize,
    backlog: Arc<Mutex<Backlog>>,
}

impl Stream for BacklogStream {
    type Item = Result<Frame, StreamError>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let mut backlog = this
            .backlog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (Some((first, first_ns)), Some((second, second_ns))) =
            (backlog.first.clone(), backlog.second.clone())
        else {
            // Both ids are recorded before the test polls anything.
            panic!("a stream was never opened");
        };
        if backlog.sent < this.total {
            backlog.sent += 1;
            return Poll::Ready(Some(Ok(Frame::Item {
                id: first,
                ns: first_ns,
                data: reading(backlog.sent as u32),
            })));
        }
        if !backlog.ended {
            backlog.ended = true;
            return Poll::Ready(Some(Ok(Frame::Item {
                id: second,
                ns: second_ns,
                data: reading(0),
            })));
        }
        Poll::Ready(None)
    }
}

/// The shared connection is one read side for every stream, so a reader that polls
/// drains it for ids it does not own. A stream nobody reads can therefore grow its
/// queue without bound while another stream keeps the socket moving (§5).
///
/// Bounded instead: the id that fell behind takes a named failure, and the frames
/// after it are dropped rather than buffered. `Internal` is the kind the codec
/// already uses for a failure the app cannot act on, and the message is the thing a
/// client can recognise and log.
#[test]
fn a_stream_left_behind_by_a_faster_reader_fails_by_name_instead_of_growing() {
    const TOTAL: usize = 500;
    let conn = Connection::new(URL, TwoStreams { total: TOTAL });
    let mut loud = now(Box::pin(conn.open(
        OP,
        serde_json::Value::Null,
        &gabion::Ctx::anonymous(),
    )))
    .expect("the first stream opens");
    let mut quiet = now(Box::pin(conn.open(
        "other.watch",
        serde_json::Value::Null,
        &gabion::Ctx::anonymous(),
    )))
    .expect("the second stream opens");

    // One poll of the quiet stream reads the socket until its own frame lands, which
    // is every one of `TOTAL` frames for the loud one.
    let quiet_frame = now(gabion::stream::next(&mut quiet));
    assert!(
        matches!(quiet_frame, Some(Ok(Frame::Item { id, .. })) if id == quiet.id()),
        "the polled stream must still be served: {quiet_frame:?}"
    );

    let loud_frame = now(gabion::stream::next(&mut loud));
    let error = match loud_frame {
        Some(Ok(Frame::Error { error, .. })) => error,
        other => {
            panic!("a stream that fell {TOTAL} frames behind must fail by name, got: {other:?}")
        }
    };
    assert_eq!(error.kind, ErrorKind::Internal, "got: {error:?}");
    assert!(
        error.message.contains("fell behind"),
        "the failure must say what happened: {}",
        error.message
    );
}
