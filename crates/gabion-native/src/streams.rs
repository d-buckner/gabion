//! The native stream transport: one WebSocket per host, written as frames
//! (`docs/streaming.md` §5).
//!
//! [`WsStreams::open`] is synchronous and a real connect is not, so opening hands
//! back a channel backed by a task that connects and then pumps both directions.
//! The channel is usable the instant it is returned; frames written before the
//! socket is up wait in its queue rather than failing.

use futures_util::{SinkExt, StreamExt};
use gabion::caps::{HttpRequest, StreamError, Streams};
use gabion::channel::{Channel, ChannelDriver, SendFrame};
use gabion::frames::Frame;
use gabion::stream::{BoxStream, Stream};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// One frame, or the reason the channel stopped carrying them.
type Received = Result<Frame, StreamError>;

/// The socket this transport dials.
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
/// The write half of one socket.
type Writer = futures_util::stream::SplitSink<Socket, Message>;
/// The read half of one socket.
type Reader = futures_util::stream::SplitStream<Socket>;

/// How many frames may sit unwritten before a producer waits.
///
/// Bounded on purpose, by the same doctrine as the server's write queue: one socket
/// is one write queue, so an `Each` producer that filled it would turn the backpressure
/// its declaration promises into memory growth (§5).
const WRITE_QUEUE: usize = 64;

/// How many frames may sit unread before the pump stops taking them off the socket.
///
/// Bounded on purpose too, and it is the sharper direction: a consumer that stops
/// polling still receives every frame the peer sends, so an unbounded queue here is
/// the same memory growth with nobody in the loop to notice. At this depth the pump
/// stops reading, the socket's own receive buffer fills, and the peer's write queue
/// backs up — §5's documented limit, reached by not reading rather than by dropping.
const READ_QUEUE: usize = 64;

/// The native `Streams`: a WebSocket to `request.url`, one task per channel.
///
/// Must be opened from inside a tokio runtime, because the pump that carries the
/// frames has to run somewhere. Outside one it reports that plainly instead of
/// handing back a channel that can never move.
#[derive(Clone, Copy, Default)]
pub struct WsStreams;

impl WsStreams {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Streams for WsStreams {
    fn open(&self, request: HttpRequest) -> Result<Channel, StreamError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            StreamError::Transport(
                "a WebSocket needs a runtime to carry it; open the channel from inside one".into(),
            )
        })?;
        let (out_tx, out_rx) = mpsc::channel(WRITE_QUEUE);
        let (in_tx, in_rx) = mpsc::channel(READ_QUEUE);
        handle.spawn(pump(socket_url(&request.url), out_rx, in_tx));
        Ok(Channel::new(WsChannel {
            out: Mutex::new(Some(out_tx)),
            incoming: Mutex::new(Some(in_rx)),
        }))
    }
}

/// Connects, then moves frames in both directions until either side stops.
///
/// The pump never reports a clean end. When the socket goes away it simply stops
/// feeding the read side, and the codec decides what a missing end means (§7.4) —
/// an adapter that turned EOF into success is the fail-open that rule bans.
///
/// The two directions are separate tasks so neither queue can hold the other: a
/// consumer that stops taking frames stalls the socket's read half (and through it
/// the peer), but a frame a caller already wrote still goes out.
async fn pump(url: String, out_rx: mpsc::Receiver<Frame>, in_tx: mpsc::Sender<Received>) {
    let socket = match tokio_tungstenite::connect_async(&url).await {
        Ok((socket, _)) => socket,
        Err(e) => {
            let _ = in_tx.send(Err(StreamError::Transport(e.to_string()))).await;
            return;
        }
    };
    let (sink, source) = socket.split();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let writing = tokio::spawn(write_frames(sink, out_rx, stopped));
    read_frames(source, &in_tx).await;
    let _ = stop.send(());
    let _ = writing.await;
}

/// The address to dial. A host configures one HTTP root and gabion builds the
/// WebSocket address from it, so `http://` becomes `ws://` and `https://`
/// becomes `wss://`; an address already spelled as a socket is left alone.
fn socket_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("ws://{rest}");
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return format!("wss://{rest}");
    }
    url.to_string()
}

/// Write frames until the write queue closes, the connection is told to stop, or a
/// write fails.
async fn write_frames(
    mut sink: Writer,
    mut out_rx: mpsc::Receiver<Frame>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        let frame = tokio::select! {
            _ = &mut stop => break,
            written = out_rx.recv() => match written {
                Some(frame) => frame,
                None => break,
            },
        };
        let Ok(text) = serde_json::to_string(&frame) else {
            break;
        };
        if sink.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
    let _ = sink.close().await;
}

/// Read frames until the socket ends, a frame will not parse, or nobody is left to
/// take them.
///
/// The send is awaited, and the queue is bounded, so a consumer that has stopped
/// polling is what stops this loop from reaching for the next frame — the socket is
/// left alone rather than drained into a queue nobody reads.
async fn read_frames(mut source: Reader, in_tx: &mpsc::Sender<Received>) {
    loop {
        let received = match source.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<Frame>(&text) {
                Ok(frame) => Ok(frame),
                Err(e) => Err(StreamError::Protocol(format!("frame would not parse: {e}"))),
            },
            Some(Ok(Message::Close(_))) | None => return,
            // Ping, pong and binary frames are not gabion's framing. A peer that
            // sends them is not speaking to this channel.
            Some(Ok(_)) => continue,
            Some(Err(e)) => Err(StreamError::Transport(e.to_string())),
        };
        let failed = received.is_err();
        if in_tx.send(received).await.is_err() {
            return;
        }
        if failed {
            return;
        }
    }
}

struct WsChannel {
    /// Taken on `close`, which ends the pump by closing its write queue.
    out: Mutex<Option<mpsc::Sender<Frame>>>,
    /// Taken once, by whoever reads the channel.
    incoming: Mutex<Option<mpsc::Receiver<Received>>>,
}

impl ChannelDriver for WsChannel {
    fn send(&self, frame: Frame) -> SendFrame<'_> {
        let sender = match self.out.lock() {
            Ok(guard) => guard
                .as_ref()
                .cloned()
                .ok_or_else(|| StreamError::Transport("the channel is closed".into())),
            Err(poisoned) => Err(StreamError::Transport(format!(
                "the channel's write queue is poisoned: {poisoned}"
            ))),
        };
        Box::pin(async move {
            // The write queue is bounded, so a producer waits for room here rather
            // than growing it — the same promise an `Each` stream makes (§5).
            match sender {
                Ok(out) => out
                    .send(frame)
                    .await
                    .map_err(|_| StreamError::Transport("the channel is closed".into())),
                Err(e) => Err(e),
            }
        })
    }

    fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>> {
        let taken = self.incoming.lock().ok().and_then(|mut slot| slot.take());
        match taken {
            Some(receiver) => Box::pin(ReceiverStream { receiver }),
            None => Box::pin(futures_util::stream::once(async {
                Err(StreamError::Protocol(
                    "a channel's read side is handed out once".into(),
                ))
            })),
        }
    }

    fn close(&self) {
        if let Ok(mut slot) = self.out.lock() {
            // Dropping the sender ends the pump's write queue, and the pump drops
            // the socket.
            slot.take();
        }
    }
}

/// The read side of a channel, over tokio's bounded queue.
struct ReceiverStream {
    receiver: mpsc::Receiver<Received>,
}

impl Stream for ReceiverStream {
    type Item = Result<Frame, StreamError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}
