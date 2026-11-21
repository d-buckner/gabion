//! The scripted stream capability, proven end to end: `MockStreams` hands out a
//! `Channel`, the codec turns that channel into a typed stream, and each shape
//! S14 demands behaves differently from the others.
//!
//! Runtime-free on purpose. A stream test that needs an executor cannot run
//! inside a `*-core`, and the conformance suite runs there.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{HttpRequest, Streams};
use gabion::channel::{Items, SendFrame};
use gabion::error::{ErrorKind, FromWire, WireError};
use gabion::frames::{Frame, StreamId};
use gabion::stream::Stream;
use gabion::testing::{MockStreams, Script};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

const URL: &str = "ws://test/api/ws";
const OP: &str = "pulse.watch";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Reading {
    frame: u32,
}

#[derive(Debug, PartialEq)]
struct AppError(String);

impl FromWire for AppError {
    fn from_wire(wire: WireError) -> Self {
        AppError(format!("{}:{}", wire.kind.as_str(), wire.message))
    }
}

fn reading(frame: u32) -> serde_json::Value {
    serde_json::to_value(Reading { frame }).unwrap()
}

/// Resolves a future that is ready in one poll. Everything the mock does on a
/// write happens in the call itself, so nothing here needs more.
fn now<T>(mut fut: impl Future<Output = T> + Unpin) -> T {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(&mut fut).poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the future did not resolve in one poll"),
    }
}

/// Opens the scripted channel and sends the `open` frame a client would send.
fn subscribe(streams: &MockStreams) -> Items<Reading, AppError> {
    let channel = streams
        .open(HttpRequest::post(URL, Vec::new()))
        .expect("scripted channel");
    let frame = Frame::Open {
        id: StreamId(1),
        op: OP.into(),
        input: serde_json::Value::Null,
    };
    let sent: SendFrame<'_> = channel.send(frame);
    now(sent).expect("the write lands");
    Items::new(StreamId(1), "pulse", channel.incoming())
}

fn poll_once(stream: &mut Items<Reading, AppError>) -> Option<Result<Reading, AppError>> {
    let mut cx = Context::from_waker(Waker::noop());
    match Pin::new(&mut *stream).poll_next(&mut cx) {
        Poll::Ready(next) => next,
        Poll::Pending => None,
    }
}

fn collect(stream: &mut Items<Reading, AppError>) -> Vec<Result<Reading, AppError>> {
    let mut out = Vec::new();
    while let Some(next) = poll_once(stream) {
        out.push(next);
    }
    out
}

fn scripted(script: Script) -> MockStreams {
    MockStreams::new().script(URL, script)
}

#[test]
fn a_scripted_stream_delivers_its_items_and_a_clean_end() {
    let streams = scripted(Script::items(vec![reading(1), reading(2)]));
    let mut stream = subscribe(&streams);
    assert_eq!(
        collect(&mut stream),
        vec![Ok(Reading { frame: 1 }), Ok(Reading { frame: 2 })]
    );
}

/// S14, the failure shape: the kind the server named is the kind the caller
/// recovers, not `Internal`.
#[test]
fn a_scripted_mid_stream_failure_arrives_as_its_named_kind() {
    for kind in [
        ErrorKind::NotFound,
        ErrorKind::Conflict,
        ErrorKind::Forbidden,
        ErrorKind::Timeout,
    ] {
        let streams = scripted(Script::failing(
            vec![reading(1)],
            WireError::new(kind, "the reader went away"),
        ));
        let mut stream = subscribe(&streams);
        assert_eq!(
            collect(&mut stream),
            vec![
                Ok(Reading { frame: 1 }),
                Err(AppError(format!("{}:the reader went away", kind.as_str()))),
            ],
            "{kind:?} must keep its own name"
        );
    }
}

/// S14, the stall shape: nothing more arrives, and nothing claims the stream
/// ended. Only a deadline or a close moves it.
#[test]
fn a_scripted_stall_never_ends_and_never_claims_one() {
    let streams = scripted(Script::stalling(vec![reading(1)]));
    let mut stream = subscribe(&streams);
    assert_eq!(
        poll_once(&mut stream),
        Some(Ok(Reading { frame: 1 })),
        "the items before the stall still arrive"
    );
    assert_eq!(
        poll_once(&mut stream),
        None,
        "a stall is not an end: it must not report a failure or a completion"
    );
}

/// S14 plus §7.4: a channel that dies without an `end` frame is a transport
/// failure, never a completed stream.
#[test]
fn a_scripted_death_without_an_end_frame_is_a_transport_failure() {
    let streams = scripted(Script::dying(vec![reading(1)]));
    let mut stream = subscribe(&streams);
    let got = collect(&mut stream);
    assert_eq!(got.len(), 2, "the item, then the failure: {got:?}");
    assert_eq!(got[0], Ok(Reading { frame: 1 }));
    let err = got[1].as_ref().expect_err("the death is reported");
    assert!(
        err.0.contains("stream transport") && err.0.contains("without an end"),
        "{err:?}"
    );
}

#[test]
fn a_refused_channel_fails_at_open_and_names_why() {
    let streams = MockStreams::new().refuse(URL, "the server is down");
    let err = streams
        .open(HttpRequest::post(URL, Vec::new()))
        .expect_err("refused");
    assert!(err.to_string().contains("the server is down"), "{err}");
}

#[test]
fn an_unscripted_channel_is_an_error_not_an_empty_stream() {
    let err = MockStreams::new()
        .open(HttpRequest::post(URL, Vec::new()))
        .expect_err("nobody scripted this endpoint");
    assert!(err.to_string().contains(URL), "{err}");
}

/// A test has to be able to see what its code sent, the same way `MockHttp` hands
/// back what was sent over it.
#[test]
fn the_capability_records_the_open_frame_and_the_request_behind_it() {
    let streams = scripted(Script::items(Vec::new()));
    drop(subscribe(&streams));
    assert_eq!(streams.opened().len(), 1, "one channel, one request");
    assert_eq!(streams.opened()[0].url, URL);
    assert_eq!(
        streams.written(),
        vec![Frame::Open {
            id: StreamId(1),
            op: OP.into(),
            input: serde_json::Value::Null,
        }],
        "the frame the client wrote, in the shape the server reads it"
    );
}

/// A client that closes its own stream stops reading it. The codec still refuses
/// to call that a clean end, because the server never said so (§7.4).
#[test]
fn a_closed_channel_stops_without_claiming_a_clean_end() {
    let streams = scripted(Script::stalling(vec![reading(1)]));
    let channel = streams
        .open(HttpRequest::post(URL, Vec::new()))
        .expect("scripted channel");
    now(channel.send(Frame::Open {
        id: StreamId(1),
        op: OP.into(),
        input: serde_json::Value::Null,
    }))
    .expect("the write lands");
    let mut stream = Items::new(StreamId(1), "pulse", channel.incoming());
    assert_eq!(poll_once(&mut stream), Some(Ok(Reading { frame: 1 })));
    now(channel.send(Frame::Close { id: StreamId(1) })).expect("the write lands");
    let after = poll_once(&mut stream).expect("the codec answers");
    assert!(
        after
            .as_ref()
            .expect_err("no end frame was sent")
            .0
            .contains("without an end"),
        "{after:?}"
    );
}
