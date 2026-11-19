//! The wasm host's wiring, run natively. Two features, one store, and the
//! numbers on the board arrive through the contract each feature publishes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use gabion::prelude::{HttpMethod, HttpResponse};
use gabion::testing::{MockHttp, TestCaps};

fn ok() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: b"{}".to_vec(),
    }
}

/// The api answers, the cdn is not scripted, so it never answers. One green row
/// and one red row is the whole point: the red row's uptime and its open incident
/// both came out of `incidents`, which only knows what `checks` published.
#[tokio::test]
async fn one_watch_is_up_and_one_is_in_an_incident() {
    let http = MockHttp::new().route(HttpMethod::Get, "http://api.internal/health", ok());
    let rows = host_pulse_web::board(TestCaps::new().with_http(http)).await;

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("api".to_string(), true, 100, false));
    assert_eq!(rows[1], ("cdn".to_string(), false, 0, true));
}
