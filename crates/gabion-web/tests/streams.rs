//! What `WebStreams` does outside a browser.
//!
//! The wasm half of this transport needs a real `WebSocket`, so the browser
//! example is its proof. Off a browser the capability has one job: say it is not
//! in a browser, and name the URL it would have dialled.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::caps::{HttpRequest, StreamError, Streams};
use gabion_web::WebStreams;

#[test]
fn a_build_that_is_not_a_browser_names_the_url_it_cannot_dial() {
    let url = "wss://example.invalid/api/ws";
    let error = WebStreams::new()
        .open(HttpRequest::get(url))
        .expect_err("no browser, so no socket");
    let StreamError::Transport(reason) = error else {
        panic!("a channel that never opened is a transport failure: {error:?}");
    };
    assert!(reason.contains(url), "the reason names the url: {reason}");
    assert!(reason.contains("browser"), "and says why: {reason}");
}
