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

/// A namespace's call queue is bounded too (§5). The calls that do not fit are
/// refused rather than queued or grown: the read loop cannot wait for room, because
/// it is also the one place a `close` is answered (§7.4).
///
/// `Internal` is the kind for a limit gabion imposed on itself, and the message
/// names the queue so a client can tell this from a slow op.
#[tokio::test]
async fn a_namespace_saturated_with_calls_refuses_the_overflow_by_name() {
    const BURST: usize = 100;
    // A stream that never ends keeps the namespace wall open for the whole burst.
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(50)));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    let _rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // Each op holds `drain_calls` for 15ms, so a burst this size cannot all fit in
    // the queue plus the one in flight.
    let mut tasks = Vec::new();
    for _ in 0..BURST {
        let conn = conn.clone();
        tasks.push(tokio::spawn(async move {
            conn.call("alpha.slow", json!({ "ms": 15 }), &Ctx::anonymous())
                .await
        }));
    }
    let mut ran = 0;
    let mut refused = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => ran += 1,
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::Internal, "got: {e:?}");
                assert!(
                    e.message.contains("call queue is full"),
                    "the refusal must say why: {}",
                    e.message
                );
                refused += 1;
            }
        }
    }
    assert_eq!(ran + refused, BURST, "every call got one answer");
    assert!(
        refused > 0,
        "a burst of {BURST} calls into a bounded queue must refuse the overflow: {ran} ran"
    );
}
