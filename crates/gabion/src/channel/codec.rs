//! The codec that turns one stream's frames into typed items, and refuses
//! anything that is not that stream's.

use crate::error::{ErrorKind, FromWire, WireError};
use crate::frames::{Frame, StreamError, StreamId};
use crate::stream::{BoxStream, Stream};
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

/// The typed view of one stream's frames.
///
/// Construct it with the frames already routed to `id`: the surface owns the
/// open-stream registry and does that routing (§5). Feeding it a connection's raw
/// frame stream would make it swallow every other stream's frames, and a frame
/// that turns out to belong to someone else is refused by name rather than
/// dropped.
///
/// The stream ends the frame source as soon as it ends the stream — after an
/// `end`, after an `error`, or after a refusal.
pub struct Items<T, E> {
    id: StreamId,
    ns: String,
    frames: BoxStream<'static, Result<Frame, StreamError>>,
    finished: bool,
    detail: Marker<T, E>,
}

/// `fn() -> (T, E)` rather than `(T, E)`: a `PhantomData` of the plain shape would
/// make `Items` `!Unpin` for any `T` or `E` that is not.
type Marker<T, E> = PhantomData<fn() -> (T, E)>;

impl<T, E> Items<T, E> {
    /// Watch stream `id`, whose items arrive under namespace `ns`.
    pub fn new(
        id: StreamId,
        ns: impl Into<String>,
        frames: BoxStream<'static, Result<Frame, StreamError>>,
    ) -> Self {
        Self {
            id,
            ns: ns.into(),
            frames,
            finished: false,
            detail: PhantomData,
        }
    }
}

impl<T: DeserializeOwned, E: FromWire> Items<T, E> {
    /// Ends the stream with a failure the caller can recover from.
    fn stop(&mut self, wire: WireError) -> Poll<Option<Result<T, E>>> {
        self.finished = true;
        Poll::Ready(Some(Err(E::from_wire(wire))))
    }

    /// One frame, one answer. Every frame shape a server may send ends the stream
    /// or yields an item, so a poll never has to read twice.
    fn on_frame(&mut self, frame: Frame) -> Poll<Option<Result<T, E>>> {
        match frame {
            Frame::Item { id, ns, data } => self.on_item(id, ns, data),
            Frame::End { .. } => {
                self.finished = true;
                Poll::Ready(None)
            }
            Frame::Error { error, .. } => self.stop(error),
            other => self.stop(
                StreamError::Protocol(format!(
                    "{other:?} is not a frame a server sends to a stream"
                ))
                .to_wire(),
            ),
        }
    }

    fn on_item(
        &mut self,
        id: StreamId,
        ns: String,
        data: serde_json::Value,
    ) -> Poll<Option<Result<T, E>>> {
        if id != self.id || ns != self.ns {
            // The namespace wall, client side (§5).
            return self.stop(WireError::new(
                ErrorKind::Forbidden,
                format!(
                    "item for stream {id} in namespace {ns} on a stream opened as {} in {}",
                    self.id, self.ns
                ),
            ));
        }
        match serde_json::from_value(data) {
            Ok(item) => Poll::Ready(Some(Ok(item))),
            Err(e) => self.stop(WireError::new(
                ErrorKind::Internal,
                format!("item did not parse: {e}"),
            )),
        }
    }
}

impl<T: DeserializeOwned, E: FromWire> Stream for Items<T, E> {
    type Item = Result<T, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        match this.frames.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            // The channel died with no `end` and no `error`. Reading that as a
            // completed stream is the fail-open §7.4 bans.
            Poll::Ready(None) => this.stop(
                StreamError::Transport("the channel closed without an end or error frame".into())
                    .to_wire(),
            ),
            Poll::Ready(Some(Err(e))) => this.stop(e.to_wire()),
            Poll::Ready(Some(Ok(frame))) => this.on_frame(frame),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the frame that broke
mod tests {
    use super::*;
    use crate::stream::EndReason;
    use std::collections::VecDeque;
    use std::task::Waker;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Reading {
        frame: u32,
    }

    #[derive(Debug, PartialEq)]
    struct AppError(String);

    impl crate::error::FromWire for AppError {
        fn from_wire(wire: WireError) -> Self {
            AppError(format!("{}:{}", wire.kind.as_str(), wire.message))
        }
    }

    /// Frames from a list, with no runtime: gabion is runtime-free, and a codec
    /// test that needed an executor could not run inside a `*-core`.
    struct Script {
        frames: VecDeque<Result<Frame, StreamError>>,
    }

    impl Stream for Script {
        type Item = Result<Frame, StreamError>;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.frames.pop_front())
        }
    }

    /// A source that never answers. The stall case, kept separate from the dead
    /// case because the codec must not confuse them.
    struct Silent;

    impl Stream for Silent {
        type Item = Result<Frame, StreamError>;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Pending
        }
    }

    fn codec(frames: Vec<Result<Frame, StreamError>>) -> Items<Reading, AppError> {
        Items::new(
            StreamId(1),
            "pulse",
            Box::pin(Script {
                frames: frames.into(),
            }),
        )
    }

    fn item(frame: u32) -> Result<Frame, StreamError> {
        Ok(Frame::Item {
            id: StreamId(1),
            ns: "pulse".into(),
            data: serde_json::to_value(Reading { frame }).expect("json"),
        })
    }

    /// Drives a stream one step, with no runtime: gabion is runtime-free, and a
    /// codec test that needed an executor could not run inside a `*-core`.
    fn poll_once(stream: &mut Items<Reading, AppError>) -> Option<Result<Reading, AppError>> {
        let mut cx = Context::from_waker(Waker::noop());
        match Pin::new(&mut *stream).poll_next(&mut cx) {
            Poll::Ready(next) => next,
            Poll::Pending => None,
        }
    }

    fn collect(mut stream: Items<Reading, AppError>) -> Vec<Result<Reading, AppError>> {
        let mut out = Vec::new();
        while let Some(next) = poll_once(&mut stream) {
            out.push(next);
        }
        out
    }

    #[test]
    fn items_decode_in_order_and_a_stated_end_is_a_clean_end() {
        let got = collect(codec(vec![
            item(1),
            item(2),
            Ok(Frame::End {
                id: StreamId(1),
                reason: EndReason::Done,
            }),
            item(3),
        ]));
        assert_eq!(
            got,
            vec![Ok(Reading { frame: 1 }), Ok(Reading { frame: 2 }),],
            "nothing is read past a stated end"
        );
    }

    /// §7.4: the framing has to *prove* the end. A channel that stops without an
    /// `end` or an `error` is a transport failure, never a completed stream.
    #[test]
    fn a_stream_that_dies_without_an_end_frame_is_a_transport_failure() {
        let got = collect(codec(vec![item(1)]));
        assert_eq!(got.len(), 2, "the item, then the failure: {got:?}");
        assert_eq!(got[0], Ok(Reading { frame: 1 }));
        let err = got[1].as_ref().expect_err("the death is reported");
        assert!(
            err.0.contains("stream transport") && err.0.contains("without an end"),
            "named as a transport failure: {err:?}"
        );
    }

    /// The same rule, when the driver reports the death instead of just stopping.
    #[test]
    fn a_channel_that_reports_its_own_failure_reaches_the_caller() {
        let got = collect(codec(vec![
            item(1),
            Err(StreamError::Transport("socket reset".into())),
        ]));
        assert_eq!(got.len(), 2);
        assert!(
            got[1]
                .as_ref()
                .expect_err("the failure")
                .0
                .contains("socket reset"),
            "{:?}",
            got[1]
        );
    }

    /// S7: a failure that crossed the wire comes back as the kind it was sent as,
    /// not as `Internal`.
    #[test]
    fn a_stream_error_round_trips_to_a_typed_error_kind() {
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Forbidden,
            ErrorKind::Timeout,
        ] {
            let got = collect(codec(vec![
                item(1),
                Ok(Frame::Error {
                    id: StreamId(1),
                    error: WireError::new(kind, "no such reading"),
                }),
                item(2),
            ]));
            assert_eq!(got.len(), 2, "nothing is read past an error: {got:?}");
            assert_eq!(
                got[1].as_ref().expect_err("the error frame arrives"),
                &AppError(format!("{}:no such reading", kind.as_str())),
                "the kind survives the round trip"
            );
        }
    }

    /// §5, client side: a frame from another namespace is refused by name.
    #[test]
    fn a_frame_from_another_namespace_is_refused_not_ignored() {
        let got = collect(codec(vec![Ok(Frame::Item {
            id: StreamId(1),
            ns: "todo".into(),
            data: serde_json::to_value(Reading { frame: 9 }).expect("json"),
        })]));
        assert_eq!(
            got,
            vec![Err(AppError(
                "forbidden:item for stream 1 in namespace todo on a stream opened as 1 in pulse"
                    .into()
            ))]
        );
    }

    #[test]
    fn a_frame_for_another_stream_is_refused_not_ignored() {
        let got = collect(codec(vec![Ok(Frame::Item {
            id: StreamId(2),
            ns: "pulse".into(),
            data: serde_json::to_value(Reading { frame: 9 }).expect("json"),
        })]));
        assert_eq!(got.len(), 1);
        assert!(
            got[0]
                .as_ref()
                .expect_err("refused")
                .0
                .starts_with("forbidden:"),
            "{:?}",
            got[0]
        );
    }

    #[test]
    fn an_item_that_is_not_the_declared_type_is_an_error_not_a_skip() {
        let got = collect(codec(vec![Ok(Frame::Item {
            id: StreamId(1),
            ns: "pulse".into(),
            data: serde_json::json!("not a reading"),
        })]));
        assert_eq!(got.len(), 1);
        assert!(
            got[0]
                .as_ref()
                .expect_err("the item did not parse")
                .0
                .starts_with("internal:item did not parse"),
            "{:?}",
            got[0]
        );
    }

    #[test]
    fn a_stalled_stream_yields_nothing_and_never_claims_an_end() {
        let mut stalled = Items::new(StreamId(1), "pulse", Box::pin(Silent));
        assert_eq!(
            poll_once(&mut stalled),
            None,
            "a channel that has said nothing has also not ended"
        );
    }
}
