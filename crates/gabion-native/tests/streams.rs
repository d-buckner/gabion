//! The native WebSocket transport, proven against a real socket: a server that
//! speaks gabion's frames, and a client that reads them through the codec.
//!
//! This is the one place the frame envelope is checked against a peer that is not
//! `MockStreams`, so the JSON a native host writes is proven to be the JSON a
//! native host reads.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures_util::{SinkExt, StreamExt};
use gabion::caps::{HttpRequest, Streams};
use gabion::channel::{Channel, Items};
use gabion::error::{FromWire, WireError};
use gabion::frames::{Frame, StreamId};
use gabion::stream::EndReason;
use gabion_native::WsStreams;
use std::net::SocketAddr;
use tokio::net::TcpListener;

const OP: &str = "pulse.watch";

#[derive(Debug, PartialEq)]
struct AppError(String);

impl FromWire for AppError {
    fn from_wire(wire: WireError) -> Self {
        AppError(format!("{}:{}", wire.kind.as_str(), wire.message))
    }
}

/// A one-connection WebSocket server: it reads the client's `open` frame, sends
/// `script`, and hangs up.
///
/// Returns the address to dial and the frames the server read, so a test can see
/// what its client actually sent.
async fn serve(script: Vec<Frame>) -> (String, tokio::task::JoinHandle<Vec<Frame>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("one connection");
        let mut ws = tokio_tungstenite::accept_async(socket)
            .await
            .expect("an upgrade");
        let mut read = Vec::new();
        match ws.next().await {
            Some(Ok(msg)) => read.push(
                serde_json::from_str::<Frame>(&msg.into_text().unwrap())
                    .expect("a frame the client sent"),
            ),
            other => panic!("the client never opened: {other:?}"),
        }
        for frame in script {
            let text = serde_json::to_string(&frame).unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                .await
                .expect("the frame reaches the client");
        }
        let _ = ws.close(None).await;
        read
    });
    (format!("ws://{addr}/api/ws"), handle)
}

/// Opens a channel, sends the `open` frame, and hands back the codec over it.
///
/// The channel is returned with the stream: dropping it closes the socket, and a
/// stream whose socket is gone is not a stream.
fn subscribe(streams: &WsStreams, url: &str) -> (Channel, Items<u32, AppError>) {
    let channel = streams.open(HttpRequest::post(url, Vec::new())).unwrap();
    channel
        .send(Frame::open(
            StreamId(1),
            OP,
            serde_json::Value::Null,
            &gabion::Ctx::anonymous(),
        ))
        .await_ok();
    let items = Items::new(StreamId(1), "pulse", channel.incoming());
    (channel, items)
}

/// Polls a send to completion. A channel write is expected to resolve at once.
trait AwaitNow {
    fn await_ok(self);
}

impl<'a> AwaitNow for gabion::channel::SendFrame<'a> {
    fn await_ok(mut self) {
        let waker = futures_util::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(
            matches!(self.as_mut().poll(&mut cx), std::task::Poll::Ready(Ok(()))),
            "a channel write queues and resolves immediately"
        );
    }
}

fn reading(frame: u32) -> serde_json::Value {
    serde_json::json!(frame)
}

#[tokio::test]
async fn a_frame_stream_round_trips_over_a_real_socket() {
    let (url, server) = serve(vec![
        Frame::Item {
            id: StreamId(1),
            ns: "pulse".into(),
            data: reading(1),
        },
        Frame::Item {
            id: StreamId(1),
            ns: "pulse".into(),
            data: reading(2),
        },
        Frame::End {
            id: StreamId(1),
            reason: EndReason::Done,
        },
    ])
    .await;

    let (_channel, mut stream) = subscribe(&WsStreams::new(), &url);
    let mut got = Vec::new();
    while let Some(next) = stream.next().await {
        got.push(next);
    }
    assert_eq!(
        got,
        vec![Ok(1), Ok(2)],
        "the items arrive, in order, and a stated end ends the stream"
    );

    let read = server.await.unwrap();
    assert_eq!(
        read,
        vec![Frame::open(
            StreamId(1),
            OP,
            serde_json::Value::Null,
            &gabion::Ctx::anonymous()
        )],
        "the server read exactly the open frame"
    );
}

/// §7.4 against a real peer: a socket that hangs up without an `end` frame is a
/// transport failure, not a completed stream.
#[tokio::test]
async fn a_socket_that_hangs_up_without_an_end_frame_is_a_transport_failure() {
    let (url, _server) = serve(vec![Frame::Item {
        id: StreamId(1),
        ns: "pulse".into(),
        data: reading(1),
    }])
    .await;

    let (_channel, mut stream) = subscribe(&WsStreams::new(), &url);
    assert_eq!(stream.next().await, Some(Ok(1)));
    let err = stream
        .next()
        .await
        .expect("the hang-up is reported")
        .expect_err("no end frame was sent");
    assert!(
        err.0.contains("stream transport"),
        "named as a transport failure: {err:?}"
    );
}

/// A frame the codec must refuse, sent by a peer that has no business sending it.
#[tokio::test]
async fn a_frame_from_another_namespace_is_refused_over_a_real_socket() {
    let (url, _server) = serve(vec![Frame::Item {
        id: StreamId(1),
        ns: "todo".into(),
        data: reading(9),
    }])
    .await;

    let (_channel, mut stream) = subscribe(&WsStreams::new(), &url);
    let err = stream
        .next()
        .await
        .expect("the frame is refused")
        .expect_err("not this stream's namespace");
    assert_eq!(
        err,
        AppError(
            "forbidden:item for stream 1 in namespace todo on a stream opened as 1 in pulse".into()
        )
    );
}

/// `close` stops the pump, and the socket goes with it.
#[tokio::test]
async fn closing_a_channel_ends_its_read_side() {
    let (url, _server) = serve(Vec::new()).await;

    let streams = WsStreams::new();
    let channel = streams.open(HttpRequest::post(&url, Vec::new())).unwrap();
    channel
        .send(Frame::open(
            StreamId(1),
            OP,
            serde_json::Value::Null,
            &gabion::Ctx::anonymous(),
        ))
        .await_ok();
    channel.close();
    // The write queue is gone, so nothing more can be sent on it.
    let err = channel
        .send(Frame::Close { id: StreamId(1) })
        .await
        .expect_err("a closed channel refuses a write");
    assert!(err.to_string().contains("closed"), "{err}");
}

/// Opening needs a runtime to carry the socket. Without one the capability says
/// so instead of handing back a channel that can never move.
#[test]
fn opening_outside_a_runtime_says_so() {
    let err = WsStreams::new()
        .open(HttpRequest::post("ws://127.0.0.1:1/api/ws", Vec::new()))
        .expect_err("no runtime here");
    assert!(err.to_string().contains("runtime"), "{err}");
}
