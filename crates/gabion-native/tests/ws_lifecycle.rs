//! What a stream's ending does to the wall and to the record behind it
//! (`docs/streaming.md` §5).
//!
//! §5's namespace wall is "is a stream live in this namespace", not "was this
//! namespace ever opened". That distinction only shows up after a stream ends
//! normally, so it gets its own file rather than hiding at the bottom of the wall
//! suite — along with the registry record that has to leave with it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "ws/harness.rs"]
mod harness;

use harness::*;

/// §5's wall is "is a stream live in this namespace", not "was it ever opened".
///
/// A listen op that finished has left the registry, so the namespace it used is
/// closed to `call` frames again. The refusal names the namespace: a client that
/// cannot tell "you never opened this" from "that stream is over" would go on
/// believing its calls are being served.
#[tokio::test]
async fn a_call_after_the_last_stream_ended_is_refused() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let mut rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let (items, end) = drain(&mut rx).await;
    assert_eq!(items.len(), 3);
    assert_eq!(end, "done");
    let error = match conn.call("alpha.ping", json!({}), &Ctx::anonymous()).await {
        Err(e) => e,
        Ok(ok) => panic!("a finished stream may not carry a call, got {ok}"),
    };
    assert_eq!(error.kind, ErrorKind::Forbidden, "got: {error:?}");
    assert!(
        error.message.contains("alpha"),
        "the refusal must name the namespace, got: {}",
        error.message
    );
}

/// The record is retired with the stream, so the id it used is free again.
///
/// This is the leak made visible from the wire: a `StreamId` held for the life of
/// the connection keeps refusing its own reuse (a second `open` under a taken id
/// is `Conflict` now), and the map grows with every stream the client ever opened.
#[tokio::test]
async fn an_id_is_free_again_after_its_stream_ends() {
    let (base, _, _) = serve().await;
    let mut socket = raw(&base).await;
    let id = StreamId(9);
    send_raw(
        &mut socket,
        Frame::open(id, "alpha.watch", Value::Null, &Ctx::anonymous()),
    )
    .await;
    let mut items = 0;
    loop {
        match next_raw(&mut socket).await {
            Frame::Item { id: seen, .. } => {
                assert_eq!(seen, id);
                items += 1;
            }
            Frame::End { id: seen, reason } => {
                assert_eq!(seen, id);
                assert_eq!(reason, EndReason::Done);
                break;
            }
            other => panic!("the first stream must end cleanly, got {other:?}"),
        }
    }
    assert_eq!(items, 3);
    // The same id names a different stream now: nothing in the protocol says an id
    // is spent for the connection's life, and the registry no longer holds it.
    send_raw(
        &mut socket,
        Frame::open(id, "beta.watch", Value::Null, &Ctx::anonymous()),
    )
    .await;
    let mut beta = 0;
    loop {
        match next_raw(&mut socket).await {
            Frame::Item { id: seen, ns, .. } => {
                assert_eq!((seen, ns.as_str()), (id, "beta"));
                beta += 1;
            }
            Frame::End { .. } => break,
            other => panic!("the id was not free again after its stream ended: {other:?}"),
        }
    }
    assert_eq!(beta, 2);
}
