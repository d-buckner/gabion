//! The kind each stream-surface refusal carries (`docs/streaming.md` §5).
//!
//! A client that retries on one kind and alerts on another cannot recover from a
//! wrong answer, so the surface answers an unknown op the way HTTP, MCP and the CLI
//! do (`NotFound`) and a malformed frame as the caller's mistake (`InvalidInput`).
//! The wall's own answer, `Forbidden`, stays in `ws.rs` with the wall.
//!
//! Split from `ws.rs` because this is one rule — "a refusal names its kind" — and
//! the wall suite had grown past the length the gate allows for one file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "ws/harness.rs"]
mod harness;

use harness::*;

#[tokio::test]
async fn a_call_for_an_unknown_op_is_not_found() {
    // Kept live for the same reason: a call into a namespace whose last stream has
    // ended is refused by the wall, and this test is about the answer an unknown op
    // gets once the wall has nothing to say.
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(20)));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    let _rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // The namespace is open, so the wall has nothing to say: the op simply is not
    // there. Over HTTP, MCP and the CLI an unknown op is `NotFound`, and the
    // stream surface is the same answer or the error model has a hole in it.
    let error = match conn
        .call("alpha.nonesuch", json!({}), &Ctx::anonymous())
        .await
    {
        Err(e) => e,
        Ok(ok) => panic!("an unknown op must not answer, got {ok}"),
    };
    assert_eq!(error.kind, ErrorKind::NotFound, "got: {error:?}");
    assert!(
        error.message.contains("no op named"),
        "the refusal must say what is missing, got: {}",
        error.message
    );
}

#[tokio::test]
async fn an_open_for_an_op_nobody_serves_is_not_found() {
    let (base, _, _) = serve().await;
    let mut socket = raw(&base).await;
    let id = StreamId(11);
    send_raw(
        &mut socket,
        Frame::open(id, "ghost.watch", Value::Null, &Ctx::anonymous()),
    )
    .await;
    // `open` answers on the socket rather than to a caller, so the refusal is read
    // here instead of through `Connection::open`.
    match next_raw(&mut socket).await {
        Frame::Error { id: got, error } => {
            assert_eq!(got, id);
            assert_eq!(error.kind, ErrorKind::NotFound, "got: {error:?}");
            assert!(
                error.message.contains("ghost.watch"),
                "the refusal must name the op, got: {}",
                error.message
            );
        }
        other => panic!("an open for an unserved op must be refused, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unparseable_frame_is_invalid_input() {
    let (base, _, _) = serve().await;
    let mut socket = raw(&base).await;
    send_text(&mut socket, "{ not a frame").await;
    match next_raw(&mut socket).await {
        Frame::Error { id, error } => {
            // The id the client meant to name is exactly what did not parse, so the
            // refusal lands under the no-stream id.
            assert_eq!(id, StreamId(0));
            assert_eq!(error.kind, ErrorKind::InvalidInput, "got: {error:?}");
            assert!(
                error.message.contains("would not parse"),
                "the refusal must say why, got: {}",
                error.message
            );
        }
        other => panic!("an unparseable frame must be refused, got {other:?}"),
    }
}

#[tokio::test]
async fn a_server_to_client_frame_from_a_client_is_invalid_input() {
    let (base, _, _) = serve().await;
    let mut socket = raw(&base).await;
    let id = StreamId(4);
    // An `item` only ever travels server to client; a client that sends one is
    // speaking the wrong half of the protocol, which is a malformed frame rather
    // than a permission one.
    send_raw(
        &mut socket,
        Frame::Item {
            id,
            ns: "alpha".into(),
            data: json!(1),
        },
    )
    .await;
    match next_raw(&mut socket).await {
        Frame::Error { id: got, error } => {
            assert_eq!(got, id);
            assert_eq!(error.kind, ErrorKind::InvalidInput, "got: {error:?}");
            assert!(
                error.message.contains("is not a frame a client sends"),
                "the refusal must say why, got: {}",
                error.message
            );
        }
        other => panic!("a server-to-client frame must be refused, got {other:?}"),
    }
}
