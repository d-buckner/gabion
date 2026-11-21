//! The per-stream pump: how one stream's items reach the socket, under the flow
//! policy its signature chose (docs/streaming.md §7.2, §7.3).
//!
//! The two loops are the whole difference between `Each` and `Latest`, so they are
//! written next to each other and nowhere else. A host that wanted a third policy
//! would add a third loop here, not a knob at startup.

use futures_util::StreamExt as _;
use gabion::ctx::Cancel;
use gabion::error::WireError;
use gabion::frames::{Frame, StreamId};
use gabion::stream::{BoxStream, EndReason};
use gabion::ws::Opened;
use gabion::Ctx;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

/// Items one stream hands the pump, already encoded as JSON.
type Items = BoxStream<'static, Result<Value, WireError>>;

/// Carry one stream's items to the connection, under the policy its signature chose.
pub(super) async fn pump(
    id: StreamId,
    ns: String,
    opened: Opened,
    ctx: Ctx,
    out: mpsc::Sender<Frame>,
) {
    match opened.policy {
        gabion::ws::Policy::Each => lossless(id, ns, opened.items, ctx, out).await,
        gabion::ws::Policy::Latest => conflating(id, ns, opened.items, ctx, out).await,
    }
}

/// `Each<T>`: every item reaches the subscriber, or the subscriber waits (§7.3).
/// The write is awaited, so a slow subscriber backpressures the connection — the
/// documented limit of one write queue per connection, made visible rather than
/// buffered away.
async fn lossless(id: StreamId, ns: String, mut items: Items, ctx: Ctx, out: mpsc::Sender<Frame>) {
    loop {
        tokio::select! {
            _ = ctx.cancel_token().cancelled() => { end(id, EndReason::Cancelled, &out).await; return; }
            next = items.next() => match next {
                Some(Ok(data)) => {
                    if out.send(item(id, &ns, data)).await.is_err() { return; }
                }
                Some(Err(error)) => { let _ = out.send(Frame::Error { id, error }).await; return; }
                None => { end(id, how(ctx.cancel_token()), &out).await; return; }
            },
        }
    }
}

/// `Latest<T>`: one slot per stream, and an untaken value is replaced (§7.2). The
/// core never knows it is conflating — it hands over a stream and a promise about
/// how it is read.
async fn conflating(
    id: StreamId,
    ns: String,
    mut items: Items,
    ctx: Ctx,
    out: mpsc::Sender<Frame>,
) {
    // A `watch` channel is the slot: sending never blocks, and a receiver that was
    // busy sees the newest value rather than a queue of stale ones.
    let (slot, mut newest) = watch::channel(None::<Value>);
    let written = out.clone();
    let writer = tokio::spawn(async move {
        while newest.changed().await.is_ok() {
            let Some(data) = newest.borrow_and_update().clone() else {
                continue;
            };
            if written.send(item(id, &ns, data)).await.is_err() {
                return;
            }
        }
    });
    // The terminator is written after the writer has drained, not from inside the
    // loop. An `end` that reached the socket ahead of the value still sitting in the
    // slot would say the stream finished while its newest item was unwritten — §7.1
    // ordering and §7.2's promise lost in the same frame.
    let outcome = loop {
        tokio::select! {
            _ = ctx.cancel_token().cancelled() => break PumpOutcome::Stopped(how(ctx.cancel_token())),
            next = items.next() => match next {
                Some(Ok(data)) => {
                    if slot.send(Some(data)).is_err() {
                        break PumpOutcome::Stopped(EndReason::Cancelled);
                    }
                }
                Some(Err(error)) => break PumpOutcome::Failed(error),
                None => break PumpOutcome::Stopped(how(ctx.cancel_token())),
            },
        }
    };
    drop(slot);
    let _ = writer.await;
    match outcome {
        PumpOutcome::Stopped(reason) => end(id, reason, &out).await,
        PumpOutcome::Failed(error) => {
            let _ = out.send(Frame::Error { id, error }).await;
        }
    }
}

/// Why the `Latest` loop stopped, once the writer has caught up.
enum PumpOutcome {
    Stopped(EndReason),
    Failed(WireError),
}

/// Why a stream that stopped by returning `None` stopped.
///
/// A core that watches its own cancel token — which is how a producer notices a
/// reader went away — ends the stream itself, and the select's cancel branch can
/// lose the race to that. Reporting `Done` there would tell the subscriber its
/// stream finished when the truth is that its own disconnect ended it (§7.4).
fn how(cancel: &Cancel) -> EndReason {
    if cancel.is_cancelled() {
        EndReason::Cancelled
    } else {
        EndReason::Done
    }
}

pub(super) fn item(id: StreamId, ns: &str, data: Value) -> Frame {
    Frame::Item {
        id,
        ns: ns.to_string(),
        data,
    }
}

pub(super) async fn end(id: StreamId, reason: EndReason, out: &mpsc::Sender<Frame>) {
    let _ = out.send(Frame::End { id, reason }).await;
}

pub(super) async fn fail(out: &mpsc::Sender<Frame>, id: StreamId, message: &str) {
    let error = WireError::new(gabion::error::ErrorKind::Forbidden, message);
    let _ = out.send(Frame::Error { id, error }).await;
}
