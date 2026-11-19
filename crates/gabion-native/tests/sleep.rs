//! The native timer is a real timer. tokio's paused clock proves it without the
//! test waiting: the runtime advances time when every task is blocked.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{timeout, Sleep, TimedOut};
use gabion_native::TokioSleep;

#[tokio::test(start_paused = true)]
async fn tokio_sleep_waits_for_the_clock_to_reach_the_deadline() {
    let before = tokio::time::Instant::now();
    TokioSleep.sleep(5_000).await;
    assert!(before.elapsed() >= std::time::Duration::from_millis(5_000));
}

#[tokio::test(start_paused = true)]
async fn timeout_cuts_off_a_wait_that_runs_past_the_deadline() {
    let outcome = timeout(
        &TokioSleep,
        50,
        Box::pin(async move {
            TokioSleep.sleep(10_000).await;
            7
        }),
    )
    .await;

    assert_eq!(outcome, Err(TimedOut));
}

#[tokio::test(start_paused = true)]
async fn timeout_keeps_a_result_that_arrives_first() {
    let outcome = timeout(&TokioSleep, 5_000, Box::pin(async { 7 })).await;

    assert_eq!(outcome, Ok(7));
}

/// Hosts build their capability bundles before the runtime exists — `host-native`
/// opens its store first, then starts tokio. Building the client may not need a
/// runtime, or every CLI host panics at startup.
#[test]
fn reqwest_http_builds_outside_a_runtime() {
    let _client = gabion_native::ReqwestHttp::new();
}
