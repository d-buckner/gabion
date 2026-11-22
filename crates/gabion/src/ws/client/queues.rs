//! The bounded queue between one shared read side and each stream that reads it
//! (`docs/streaming.md` §5).
//!
//! One reader drains the socket for every stream id, so a stream nobody is reading
//! would grow a queue for as long as any other stream keeps the connection moving.
//! Bounded instead: an id that outruns its own reader is owed a named failure, and
//! the frames after it are dropped rather than buffered — the same choice the codec
//! makes for an item that will not parse.

use crate::error::{ErrorKind, WireError};
use crate::frames::{Frame, StreamId};
use std::collections::{HashMap, VecDeque};
use std::task::Waker;

/// How many frames may wait for one stream's reader before that stream fails.
///
/// Small enough that a stream which is not being read cannot hide much work, large
/// enough that one poll of a slow reader does not fail it.
const STREAM_QUEUE: usize = 64;

/// What a stream that outran its own reader is told.
///
/// A named failure rather than a drop, because a caller cannot act on frames that
/// silently stopped arriving: `Internal` is the kind gabion already uses for its own
/// limit, and the message is what a client logs when it reopens the stream.
const FELL_BEHIND: &str = "this stream fell behind";

/// Whether a frame is the last one a stream will see.
fn is_terminator(frame: &Frame) -> bool {
    matches!(frame, Frame::End { .. } | Frame::Error { .. })
}

/// Every stream's pending frames, the failures owed to readers, and the readers
/// waiting for one.
#[derive(Default)]
pub(super) struct Queues {
    pending: HashMap<StreamId, VecDeque<Frame>>,
    /// Streams whose queue hit its bound, and the failure each reader is owed. Held
    /// apart from `pending` so an id that fell behind cannot be reported as having
    /// produced the item it ran out of room for.
    fallen: HashMap<StreamId, WireError>,
    /// One consumer per stream id, so a frame wakes its own reader and nobody
    /// else's.
    waiting: HashMap<StreamId, Waker>,
}

impl Queues {
    /// The next frame for `id`: what arrived, or the failure that stream is owed.
    pub(super) fn take(&mut self, id: StreamId) -> Option<Frame> {
        if let Some(error) = self.fallen.remove(&id) {
            return Some(Frame::Error { id, error });
        }
        self.pending.get_mut(&id).and_then(VecDeque::pop_front)
    }

    /// Put a frame back at the front of `id`'s queue.
    ///
    /// A caller that had to look at the first frame to answer with it — the typed
    /// stream opener does — gives it back rather than consuming what the stream was
    /// promised.
    pub(super) fn restore(&mut self, id: StreamId, frame: Frame) {
        self.pending.entry(id).or_default().push_front(frame);
    }

    /// Hold one routed frame under its id, or give the id its failure instead.
    ///
    /// Called with the frame the peer sent, whoever asked for a read. An id already
    /// owed a failure gets nothing more: buffering the rest is the growth this
    /// bound exists to stop.
    pub(super) fn hold(&mut self, frame: Frame) {
        let id = frame.id();
        if !self.fallen.contains_key(&id) {
            let over = !is_terminator(&frame)
                && self
                    .pending
                    .get(&id)
                    .is_some_and(|queued| queued.len() >= STREAM_QUEUE);
            if over {
                // The frames nobody read are dropped rather than handed over late: a
                // reader that resumed here would be reading a stream the peer has
                // already moved past.
                self.pending.remove(&id);
                self.fallen
                    .insert(id, WireError::new(ErrorKind::Internal, FELL_BEHIND));
            } else {
                self.pending.entry(id).or_default().push_back(frame);
            }
        }
        self.wake(id);
    }

    /// Park `id`'s reader until its next frame lands.
    pub(super) fn park(&mut self, id: StreamId, waker: Waker) {
        self.waiting.insert(id, waker);
    }

    /// A reader that is gone is owed nothing.
    pub(super) fn forget(&mut self, id: StreamId) {
        self.fallen.remove(&id);
    }

    /// Every parked reader, left to be woken by the caller.
    pub(super) fn waiting(&mut self) -> Vec<Waker> {
        self.waiting.drain().map(|(_, waker)| waker).collect()
    }

    fn wake(&mut self, id: StreamId) {
        if let Some(waker) = self.waiting.remove(&id) {
            waker.wake();
        }
    }
}
