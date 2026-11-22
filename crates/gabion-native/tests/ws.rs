//! The stream surface over a real socket: the namespace wall, the ordering rule,
//! and what a disconnect does to a producer (`docs/streaming.md` §5, §7).
//!
//! Driven through `ws_router` on a real listener, with a real client `Connection`
//! on the other end. A unit test of the pump would prove the pump; it would not
//! prove that a frame one feature wrote reaches the reader that named it, which is
//! what the wall and the multiplexer have to get right together.
//!
//! The flow-policy cases live in `ws_flow.rs`; the harness both files need lives in
//! `ws/harness.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "ws/harness.rs"]
mod harness;

use harness::*;

#[tokio::test]
async fn a_frame_in_another_features_namespace_is_refused() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let _rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // A `beta` op on a connection that only ever opened `alpha`.
    let error = match conn.call("beta.ping", json!({}), &Ctx::anonymous()).await {
        Err(e) => e,
        Ok(ok) => panic!("a call outside the opened namespace must not answer, got {ok}"),
    };
    assert_eq!(error.kind, ErrorKind::Forbidden, "got: {error:?}");
    assert!(
        error.message.contains("outside every namespace"),
        "the refusal must say why, got: {}",
        error.message
    );
}

#[tokio::test]
async fn a_feature_never_receives_another_features_frames() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let mut alpha = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let mut beta = conn
        .open("beta.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // Both streams are live on one socket at once, so every frame that arrives has
    // to be routed. A reader that took a neighbour's frame would show it here.
    let (alpha_items, alpha_end) = drain(&mut alpha).await;
    let (beta_items, beta_end) = drain(&mut beta).await;
    assert_eq!(
        alpha_items.len(),
        3,
        "alpha has three items: {alpha_items:?}"
    );
    assert_eq!(beta_items.len(), 2, "beta has two items: {beta_items:?}");
    assert_eq!(alpha_end, "done");
    assert_eq!(beta_end, "done");
    for item in &alpha_items {
        assert!(item.get("n").is_some(), "alpha item is not alpha's: {item}");
    }
}

#[tokio::test]
async fn a_client_may_not_close_a_stream_it_did_not_open() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let mine = rx.id();
    let other = StreamId(mine.0 + 500);
    let error = match conn.close(other).await {
        Err(e) => e,
        Ok(reason) => panic!("closing a stream nobody opened must fail, got {reason:?}"),
    };
    assert!(
        error.message.contains("was not opened"),
        "the refusal must name the rule, got: {}",
        error.message
    );
    // And the stream it did open is still there, unread and unharmed.
    drop(rx);
}

#[tokio::test]
async fn a_second_open_under_a_live_id_is_refused() {
    // The stream must still be running when the second `open` arrives, or the id is
    // free again and there is nothing to collide with. A slow tick means the first
    // pump is parked rather than finished.
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(100)));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let mut socket = raw(&base).await;
    let live = StreamId(7);
    send_raw(
        &mut socket,
        Frame::open(live, "alpha.watch", Value::Null, &Ctx::anonymous()),
    )
    .await;
    // The first stream is answering under this id, so the id is taken.
    match next_raw(&mut socket).await {
        Frame::Item { id, ns, .. } => {
            assert_eq!(id, live);
            assert_eq!(ns, "alpha", "the live stream is alpha's");
        }
        other => panic!("the first stream must produce an item, got {other:?}"),
    }
    // A second `open` under the same id used to replace the record and orphan the
    // pump behind it: two producers writing items under one id, indistinguishable
    // to the client. A taken id is refused, the way an unknown id is on `close`.
    send_raw(
        &mut socket,
        Frame::open(live, "beta.watch", Value::Null, &Ctx::anonymous()),
    )
    .await;
    loop {
        match next_raw(&mut socket).await {
            Frame::Item { id, ns, .. } => assert_eq!(
                (id, ns.as_str()),
                (live, "alpha"),
                "only the first stream may write under the id"
            ),
            Frame::Error { id, error } => {
                assert_eq!(id, live);
                assert_eq!(error.kind, ErrorKind::Conflict, "got: {error:?}");
                assert!(
                    error.message.contains('7'),
                    "the refusal must name the id, got: {}",
                    error.message
                );
                break;
            }
            other => panic!("a second open must be refused, got {other:?}"),
        }
    }
    // And the stream that owns the id is unharmed: its pump still writes.
    match next_raw(&mut socket).await {
        Frame::Item { id, ns, .. } => {
            assert_eq!((id, ns.as_str()), (live, "alpha"));
        }
        other => panic!("the first pump was clobbered by the second open: {other:?}"),
    }
}

#[tokio::test]
async fn two_sends_keep_their_order_on_one_connection() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let _rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // One fast call and one slow call, issued in that order on one namespace. The
    // slow one must not answer first, so the replies are stamped as they land.
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for (op, ms) in [("alpha.ping", 0u64), ("alpha.slow", 120)] {
        let conn = conn.clone();
        let order = Arc::clone(&order);
        handles.push(tokio::spawn(async move {
            let answer = conn
                .call(op, json!({ "ms": ms }), &Ctx::anonymous())
                .await
                .unwrap();
            order
                .lock()
                .unwrap()
                .push(answer["op"].as_str().unwrap().to_string());
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(
        *order.lock().unwrap(),
        vec!["alpha.ping".to_string(), "alpha.slow".to_string()],
        "calls on one namespace must answer in the order they arrived (§7.1)"
    );
}

#[tokio::test]
async fn a_close_is_handled_while_a_send_is_in_flight() {
    // alpha must still be producing when the close arrives, so it ticks rather
    // than finishing before the test can interrupt it.
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(20)));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    let rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let id = rx.id();
    // One reader per stream, and it is the one inside `close`: the terminator a
    // close causes arrives on the stream it closed, so a second reader here would
    // race it for the frame. The stream ticks slowly enough that nothing backs up.
    drop(rx);
    let slow = {
        let conn = conn.clone();
        tokio::spawn(async move {
            conn.call("alpha.slow", json!({ "ms": 600 }), &Ctx::anonymous())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(40)).await;
    let started = Instant::now();
    let reason = conn
        .close(id)
        .await
        .expect("the close must be answered while a call is still in flight");
    let waited = started.elapsed();
    // The call that was already in flight when the close arrived still gets its
    // answer: a close ends one stream, not the namespace's whole queue (§7.1).
    assert_eq!(
        slow.await.unwrap().unwrap()["op"].as_str().unwrap(),
        "alpha.slow"
    );
    assert!(
        waited < Duration::from_millis(400),
        "the close waited behind the in-flight call: {waited:?}"
    );
    assert_eq!(reason, EndReason::Cancelled);
}

#[tokio::test]
async fn a_close_frame_ends_one_stream_and_leaves_the_others_up() {
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(20)));
    let (base, _, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    let alpha = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let alpha_id = alpha.id();
    drop(alpha);
    let mut beta = conn
        .open("beta.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let reason = conn.close(alpha_id).await.unwrap();
    assert_eq!(reason, EndReason::Cancelled);
    // beta is untouched: its own items, its own clean end.
    let (beta_items, beta_end) = drain(&mut beta).await;
    assert_eq!(beta_items.len(), 2, "beta kept its items: {beta_items:?}");
    assert_eq!(beta_end, "done");
}

#[tokio::test]
async fn a_dropped_socket_ends_the_producer_even_while_it_is_idle() {
    // Slow enough that the producer is parked on its own timer rather than on a
    // write queue nobody is draining, which is the idle case §7.4 names.
    let alpha = Arc::new(Feature::open_ended("alpha", Duration::from_millis(100)));
    let (base, alpha, _) = serve_with(alpha, Arc::new(Feature::new("beta", 2))).await;
    let conn = client(&base);
    let rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    // Nothing is ever read, so the producer is parked on its cancel token and has
    // never touched a dead write. Only the disconnect can stop it (§7.4).
    drop(rx);
    drop(conn);
    for _ in 0..150 {
        if alpha.stopped.load(Ordering::SeqCst) > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the producer outlived the socket that opened it");
}

#[tokio::test]
async fn stream_item_order_survives_all_adapters() {
    let (base, _, _) = serve().await;
    let conn = client(&base);
    let mut rx = conn
        .open("alpha.watch", Value::Null, &Ctx::anonymous())
        .await
        .unwrap();
    let (items, end) = drain(&mut rx).await;
    assert_eq!(end, "done");
    let ns: Vec<u64> = items.iter().map(|i| i["n"].as_u64().unwrap()).collect();
    assert_eq!(ns, vec![1, 2, 3], "items must arrive in the order produced");
}
