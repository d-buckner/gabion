//! Cancellation, exercised under a real executor.
//!
//! The unit tests in `src/ctx.rs` cover the two conversions; this covers the
//! part a conversion test cannot: that a waker parked on a cancellation flag is
//! actually woken, and that the future completes every time it is polled after
//! the flag is up. A cancellation handle that hangs the second time is worse than
//! none, because it turns a cancelled call into a stuck one.
//! Needs a real executor, so it is not built for wasm32.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use gabion::prelude::Ctx;

#[tokio::test]
async fn cancelling_wakes_a_waiter_under_a_real_executor() {
    let ctx = Ctx::anonymous();
    let handle = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        handle.cancel();
    });

    tokio::time::timeout(Duration::from_secs(5), ctx.cancel_token().cancelled())
        .await
        .expect("cancelling did not wake the waiter");
    assert!(ctx.is_cancelled());
}

/// A clone is another handle to the same signal, not a copy of it: a host that
/// hands one out must still be able to cancel work it can no longer reach.
#[tokio::test]
async fn cancelling_one_handle_cancels_every_clone() {
    let ctx = Ctx::anonymous();
    let other = ctx.clone();
    other.cancel();
    assert!(ctx.is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), ctx.cancel_token().cancelled())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_cancelled_token_is_ready_on_every_poll() {
    let ctx = Ctx::anonymous();
    ctx.cancel();
    tokio::time::timeout(Duration::from_secs(5), ctx.cancel_token().cancelled())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ctx.cancel_token().cancelled())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_token_nobody_cancels_stays_pending() {
    let ctx = Ctx::anonymous();
    let outcome =
        tokio::time::timeout(Duration::from_millis(20), ctx.cancel_token().cancelled()).await;
    assert!(outcome.is_err(), "an uncancelled call must not complete");
    assert!(!ctx.is_cancelled());
}

/// Cancelling twice wakes nobody twice, and does not panic on the second call.
#[tokio::test]
async fn cancelling_twice_is_idempotent() {
    let ctx = Ctx::anonymous();
    ctx.cancel();
    ctx.cancel();
    assert!(ctx.is_cancelled());
}
