//! The per-stream pump: how one stream's items reach the socket, under the flow
//! policy its signature chose (docs/streaming.md §7.2, §7.3).
//!
//! The two loops are the whole difference between `Each` and `Latest`, so they are
//! written next to each other and nowhere else. A host that wanted a third policy
//! would add a third loop here, not a knob at startup.
//!
//! The refusals a frame handler sends live here too, because the `ErrorKind` in one
//! is part of the protocol a reader has to understand.

use super::registry::{locked, Registry};
use futures_util::StreamExt as _;
use gabion::ctx::Cancel;
use gabion::error::{ErrorKind, WireError};
use gabion::frames::{Frame, StreamId};
use gabion::stream::{BoxStream, EndReason};
use gabion::ws::Opened;
use gabion::Ctx;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

/// Items one stream hands the pump, already encoded as JSON.
type Items = BoxStream<'static, Result<Value, WireError>>;

/// Carry one stream's items to the connection, under the policy its signature chose.
///
/// The pump's last act is to give the stream's id back, after the loop that stopped
/// it — so a cancellation, a clean end and a mid-stream failure all leave through
/// one retirement instead of three. It happens *before* the terminator is written: a
/// client that reads its own `end` and sends a `call` in the same breath must find
/// the namespace wall already closed rather than race the pump's next instruction.
pub(super) async fn pump(
    id: StreamId,
    ns: String,
    opened: Opened,
    ctx: Ctx,
    out: mpsc::Sender<Frame>,
    registry: Arc<Mutex<Registry>>,
) {
    let ending = match opened.policy {
        gabion::ws::Policy::Each => lossless(id, &ns, opened.items, &ctx, &out).await,
        gabion::ws::Policy::Latest => conflating(id, ns, opened.items, &ctx, &out).await,
    };
    locked(&registry).retire(id);
    match ending {
        Ending::End(reason) => end(id, reason, &out).await,
        Ending::Failed(error) => {
            let _ = out.send(Frame::Error { id, error }).await;
        }
        // The write that would have carried the terminator already failed, so the
        // connection is gone and there is nobody left to state an ending to.
        Ending::Unheard => {}
    }
}

/// `Each<T>`: every item reaches the subscriber, or the subscriber waits (§7.3).
/// The write is awaited, so a slow subscriber backpressures the connection — the
/// documented limit of one write queue per connection, made visible rather than
/// buffered away.
async fn lossless(
    id: StreamId,
    ns: &str,
    mut items: Items,
    ctx: &Ctx,
    out: &mpsc::Sender<Frame>,
) -> Ending {
    loop {
        tokio::select! {
            _ = ctx.cancel_token().cancelled() => return Ending::End(EndReason::Cancelled),
            next = items.next() => match next {
                Some(Ok(data)) => {
                    if out.send(item(id, ns, data)).await.is_err() {
                        return Ending::Unheard;
                    }
                }
                Some(Err(error)) => return Ending::Failed(error),
                None => return Ending::End(how(ctx.cancel_token())),
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
    ctx: &Ctx,
    out: &mpsc::Sender<Frame>,
) -> Ending {
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
            _ = ctx.cancel_token().cancelled() => break Ending::End(how(ctx.cancel_token())),
            next = items.next() => match next {
                Some(Ok(data)) => {
                    // The slot's only sender is this loop, so a failed send means the
                    // writer task is gone and nothing can reach the socket again.
                    if slot.send(Some(data)).is_err() {
                        break Ending::Unheard;
                    }
                }
                Some(Err(error)) => break Ending::Failed(error),
                None => break Ending::End(how(ctx.cancel_token())),
            },
        }
    };
    drop(slot);
    let _ = writer.await;
    outcome
}

/// How a stream stopped: the one frame that states it, once the record is retired.
///
/// `Latest` needs the ending as a value, because its terminator is written after the
/// writer has drained. `Each` builds one too, so both policies state their ending at
/// the one place that has already given the stream's id back.
enum Ending {
    End(EndReason),
    Failed(WireError),
    /// The write that would have stated the ending failed; the connection is gone.
    Unheard,
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

/// Refuse one frame with the kind its caller knows.
///
/// The kind is an argument, not a constant: the site that knows what went wrong
/// names the outcome. One default for every refusal reported a permission problem
/// when a frame simply did not parse, which is the cross-surface drift the shared
/// error model exists to prevent (`docs/streaming.md` §5).
pub(super) async fn fail(out: &mpsc::Sender<Frame>, id: StreamId, kind: ErrorKind, message: &str) {
    let error = WireError::new(kind, message);
    let _ = out.send(Frame::Error { id, error }).await;
}

/// The one answer to "no op named `op`", whoever asked.
///
/// An unknown op is `NotFound` on every surface, but `open` resolves it against the
/// mounted services and `call` writes the frame from the namespace queue. Building
/// it in one place is what keeps those two spellings the same answer.
pub(super) fn no_such_op(id: StreamId, op: &str) -> Frame {
    Frame::Error {
        id,
        error: WireError::new(ErrorKind::NotFound, format!("no op named `{op}`")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // a failing assertion names the case
mod tests {
    use super::*;
    use gabion::ws::Policy;
    use serde_json::json;

    fn opened(policy: Policy, items: Vec<Result<Value, WireError>>) -> Opened {
        Opened {
            policy,
            items: Box::pin(futures_util::stream::iter(items)),
        }
    }

    /// The record a stream claimed is given back however its pump leaves: a clean
    /// end under both policies, a mid-stream failure, and a cancellation. One map
    /// that only grows is the leak §5's wall then reads as "was this namespace ever
    /// opened", and the id stops being reusable for the connection's life.
    #[tokio::test]
    async fn a_pump_retires_its_record_on_every_way_out() {
        let cases = [
            (
                "each, ended",
                Policy::Each,
                vec![Ok(json!(1))],
                EndReason::Done,
            ),
            (
                "latest, ended",
                Policy::Latest,
                vec![Ok(json!(1))],
                EndReason::Done,
            ),
            (
                "latest, failed",
                Policy::Latest,
                vec![Err(WireError::new(ErrorKind::Internal, "boom"))],
                EndReason::Done,
            ),
        ];
        for (name, policy, items, _) in cases {
            let registry = Arc::new(Mutex::new(Registry::default()));
            let cancel = Cancel::new();
            assert!(locked(&registry).claim(StreamId(1), "alpha".into(), cancel.clone()));
            let (out, mut queue) = mpsc::channel(8);
            let ctx = Ctx::anonymous().with_cancel(cancel);
            pump(
                StreamId(1),
                "alpha".into(),
                opened(policy, items),
                ctx,
                out,
                Arc::clone(&registry),
            )
            .await;
            assert_eq!(
                locked(&registry).count(),
                0,
                "{name} left its open record behind"
            );
            assert!(
                queue.try_recv().is_ok(),
                "{name} stated no ending on the wire"
            );
        }
    }
}
