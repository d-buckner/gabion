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
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
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

async fn pump(
    url: String,
    mut out_rx: mpsc::UnboundedReceiver<Frame>,
    in_tx: mpsc::UnboundedSender<Received>,
) {
    let socket = match tokio_tungstenite::connect_async(&url).await {
        Ok((socket, _)) => socket,
        Err(e) => {
            let _ = in_tx.send(Err(StreamError::Transport(e.to_string())));
            return;
        }
    };
    let (mut sink, mut source) = socket.split();
    loop {
        tokio::select! {
            written = out_rx.recv() => {
                let Some(frame) = written else { break };
                let text = match serde_json::to_string(&frame) {
                    Ok(text) => text,
                    Err(e) => {
                        let _ = in_tx.send(Err(StreamError::Protocol(format!(
                            "frame would not serialize: {e}"
                        ))));
                        break;
                    }
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            read = source.next() => {
                match read {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<Frame>(&text) {
                        Ok(frame) => {
                            if in_tx.send(Ok(frame)).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = in_tx.send(Err(StreamError::Protocol(format!(
                                "frame would not parse: {e}"
                            ))));
                            break;
                        }
                    },
                    Some(Ok(Message::Close(_))) | None => break,
                    // Ping, pong and binary frames are not gabion's framing. A
                    // peer that sends them is not speaking to this channel.
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let _ = in_tx.send(Err(StreamError::Transport(e.to_string())));
                        break;
                    }
                }
            }
        }
    }
    let _ = sink.close().await;
}

struct WsChannel {
    /// Taken on `close`, which ends the pump by closing its write queue.
    out: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    /// Taken once, by whoever reads the channel.
    incoming: Mutex<Option<mpsc::UnboundedReceiver<Received>>>,
}

impl ChannelDriver for WsChannel {
    fn send(&self, frame: Frame) -> SendFrame<'_> {
        let outcome = match self.out.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(out) => out
                    .send(frame)
                    .map_err(|_| StreamError::Transport("the channel is closed".into())),
                None => Err(StreamError::Transport("the channel is closed".into())),
            },
            Err(poisoned) => Err(StreamError::Transport(format!(
                "the channel's write queue is poisoned: {}",
                poisoned
            ))),
        };
        Box::pin(async { outcome })
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

/// The read side of a channel, over tokio's unbounded queue.
struct ReceiverStream {
    receiver: mpsc::UnboundedReceiver<Received>,
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
