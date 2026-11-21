//! What each flow policy promises the subscriber, over a real socket
//! (`docs/streaming.md` §7.2, §7.3).
//!
//! `Each` and `Latest` differ in exactly one thing: what happens when the subscriber
//! is slower than the producer. That difference is the whole reason two markers
//! exist, so it gets its own file rather than hiding at the bottom of the wall suite.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "ws/harness.rs"]
mod harness;

use harness::*;

/// `Latest<T>` holds one slot and replaces an untaken value (§7.2).
///
/// The invariant a subscriber can rely on is not "I saw every other item" — that is
/// `Each`. It is that the newest value is never buried behind values nobody asked
/// for. So the assertion is about the last value, not about a count: a count would
/// depend on how fast this test happens to run.
#[tokio::test]
async fn a_conflating_stream_keeps_the_last_value() {
    let alpha = Arc::new(Feature::new("alpha", 6));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    // One `Latest` stream, read one item at a time with the connection left idle
    // between reads, which is the shape that lets the slot overwrite.
    let mut rx = conn
        .open("alpha.tail", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let (items, end) = drain(&mut rx).await;
    assert_eq!(end, "done");
    let seen: Vec<u64> = items.iter().map(|i| i["n"].as_u64().unwrap()).collect();
    assert!(
        seen.contains(&6),
        "the newest value must survive, saw {seen:?}"
    );
}
