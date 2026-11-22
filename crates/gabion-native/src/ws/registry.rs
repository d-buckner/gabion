//! The open-stream record one connection keeps: which streams are live, and the
//! ordered call queue each namespace joins (`docs/streaming.md` §5, §7.1).
//!
//! Two maps, with two different lifetimes, and the difference is the whole point:
//!
//! - `open` is the liveness record the namespace wall reads. A stream joins it when
//!   its `open` frame arrives and leaves it the moment its pump stops stating the
//!   stream — so "a stream in this namespace is live" is what a `call` is checked
//!   against, never "this namespace was opened once".
//! - `namespaces` holds one ordered, bounded queue per namespace for the life of
//!   the connection. It outlives every stream in it on purpose: retiring a queue when
//!   its last stream ended would drop the calls already queued behind it, which is
//!   §7.1's promise to take their turn.
//! - A full queue is a refusal, not growth ([`CALL_QUEUE`]): the read loop cannot
//!   wait for room without also waiting behind the `close` it alone can answer, so the
//!   call that does not fit is answered with `Internal` and the client can retry.

use gabion::ctx::{Cancel, Ctx};
use gabion::frames::{Frame, StreamId};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::mpsc;

use super::{no_such_op, Conn, Surface};

/// Take the open-stream registry. A panic in one connection's pump leaves the map
/// intact, so there is nothing to reconstruct and dropping the connection would
/// turn one panic into every later caller's mystery.
pub(crate) fn locked(registry: &Arc<Mutex<Registry>>) -> MutexGuard<'_, Registry> {
    match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// How many calls may wait for one namespace's queue before the next is refused.
///
/// Bounded by the same doctrine as the write queue: a client that sends `call`
/// frames faster than one `drain_calls` task runs them would otherwise grow this
/// queue for the life of the connection (§5). The read loop cannot wait for room —
/// it is also the only place a `close` is handled, so a wait here would keep a
/// producer that could be cancelled running (§7.4) — so a call that does not fit is
/// refused with `Internal` instead of being queued or dropped silently.
pub(super) const CALL_QUEUE: usize = 64;

/// Streams this connection opened. The one record the namespace wall reads.
#[derive(Default)]
pub(crate) struct Registry {
    open: HashMap<StreamId, Open>,
    /// One ordered queue per namespace, so calls in a namespace keep arrival order.
    namespaces: HashMap<String, mpsc::Sender<Call>>,
}

pub(crate) struct Open {
    ns: String,
    cancel: Cancel,
}

pub(crate) struct Call {
    pub(crate) op: String,
    pub(crate) input: Value,
    pub(crate) ctx: Ctx,
    pub(crate) id: StreamId,
}

impl Registry {
    /// Take an id for a live stream, or say it is taken.
    ///
    /// A second `open` under an id that is already live would replace the record
    /// and orphan the pump behind it: the first producer keeps writing items under
    /// an id the client now reads as the second stream's, and `Items` accepts any
    /// item whose id and namespace match, so the two cannot be told apart.
    pub(crate) fn claim(&mut self, id: StreamId, ns: String, cancel: Cancel) -> bool {
        if self.open.contains_key(&id) {
            return false;
        }
        self.open.insert(id, Open { ns, cancel });
        true
    }

    /// Give an id up because its stream stopped.
    ///
    /// Called by the pump before it states the ending, not after: a client that
    /// reads its own `end` and sends a `call` in the same breath must find the wall
    /// already closed rather than race the pump's last instruction. The namespace
    /// queue is untouched — see the module comment.
    pub(crate) fn retire(&mut self, id: StreamId) {
        self.open.remove(&id);
    }

    /// Give an id up because its client asked, stopping the producer behind it.
    ///
    /// `false` means this connection never opened that id, which is the one answer a
    /// `close` can be wrong about.
    pub(crate) fn close(&mut self, id: StreamId) -> bool {
        match self.open.remove(&id) {
            Some(open) => {
                open.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// End every stream on this connection. The socket is gone, so nothing is
    /// waiting for a terminator.
    pub(crate) fn cancel_all(&mut self) {
        for open in self.open.values() {
            open.cancel.cancel();
        }
        self.open.clear();
    }

    /// How many streams are live. Test-only: the leak §5's wall reads as "was this
    /// namespace ever opened" is a count nobody outside a test can act on.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.open.len()
    }

    /// The ordered queue for a namespace, only if a stream in it is live.
    ///
    /// The liveness check is the `open` map, and the queue is not: a namespace whose
    /// last stream ended keeps its queue so the calls already in it still take their
    /// turn (§7.1), and a later `open` in that namespace rejoins the same queue, so
    /// arrival order survives a stream ending. Only a namespace with no live stream
    /// gets no queue, which is the refusal the wall exists for.
    pub(crate) fn sender_for(&mut self, ns: &str, conn: &Conn<'_>) -> Option<mpsc::Sender<Call>> {
        if !self.open.values().any(|o| o.ns == ns) {
            return None;
        }
        if let Some(sender) = self.namespaces.get(ns) {
            return Some(sender.clone());
        }
        let (sender, receiver) = mpsc::channel(CALL_QUEUE);
        self.namespaces.insert(ns.to_string(), sender.clone());
        tokio::spawn(drain_calls(
            receiver,
            Arc::clone(conn.surface),
            conn.out.clone(),
        ));
        Some(sender)
    }
}

/// Calls for one namespace, in the order they arrived (§7.1).
///
/// Sequential within the namespace, and one task per namespace, so a slow op in one
/// feature does not hold another's. Writing the reply is awaited here rather than
/// spawned, or the second reply could reach the socket before the first.
async fn drain_calls(
    mut receiver: mpsc::Receiver<Call>,
    surface: Arc<Surface>,
    out: mpsc::Sender<Frame>,
) {
    while let Some(call) = receiver.recv().await {
        let frame = match surface.find(&call.op) {
            Some(service) => match service.call(&call.ctx, &call.op, call.input).await {
                Ok(ok) => Frame::Reply { id: call.id, ok },
                Err(error) => Frame::Error { id: call.id, error },
            },
            None => no_such_op(call.id, &call.op),
        };
        if out.send(frame).await.is_err() {
            return;
        }
    }
}
