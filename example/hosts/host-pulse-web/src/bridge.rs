//! Turn a browser-only future into one the capability traits can carry.
//!
//! [`gabion::caps::Timer`](gabion::caps::Timer) is
//! `Pin<Box<dyn Future<Output = ()> + Send>>`, and every `wasm-bindgen` future is
//! `!Send`: it pins a JS handle, which belongs to this thread and this thread
//! only. The bridge keeps the handle inside a `spawn_local` task and hands the
//! futures that await it a plain `Arc<Mutex<..>>` slot — which is `Send`, because
//! the JS value never leaves the task.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use wasm_bindgen_futures::spawn_local;

struct Slot<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

/// A `Send` future for a value produced by a JS task on this thread.
pub struct Bridged<T> {
    slot: Arc<Mutex<Slot<T>>>,
}

impl<T: Send> Future for Bridged<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut slot = lock(&self.slot);
        if let Some(value) = slot.value.take() {
            return Poll::Ready(value);
        }
        slot.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn lock<T>(slot: &Arc<Mutex<T>>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Spawn `future` on this thread and return a `Send` future for its output.
pub fn bridge<T, F>(future: F) -> Bridged<T>
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    let slot = Arc::new(Mutex::new(Slot {
        value: None,
        waker: None,
    }));
    let producer = slot.clone();
    spawn_local(async move {
        let value = future.await;
        let mut slot = lock(&producer);
        slot.value = Some(value);
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    });
    Bridged { slot }
}
