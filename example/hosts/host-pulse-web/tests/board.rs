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

/// The api answers, the offline port is not scripted, so it never answers. One
/// green row and one red row is the whole point: the red row's uptime and its
/// open incident both came out of `incidents`, which only knows what `checks`
/// published.
#[tokio::test]
async fn one_watch_is_up_and_one_is_in_an_incident() {
    let http = MockHttp::new().route(HttpMethod::Get, "https://api.github.com/zen", ok());
    let rows = host_pulse_web::board(TestCaps::new().with_http(http)).await;

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("api".to_string(), true, 100, false));
    assert_eq!(rows[1], ("offline".to_string(), false, 0, true));
}

/// The same mounting a server host does, in the browser build. Linker-free by
/// design: `linkme` has no wasm32 implementation, and a registry that silently
/// came back empty in a page would be the worst kind of green.
#[test]
fn the_page_can_name_the_operations_it_holds() {
    let ops = host_pulse_web::exposed_ops();
    assert_eq!(
        ops,
        "check.add,check.history,check.list,check.probe,check.sweep,\
incident.evaluate,incident.list,incident.uptime"
    );
}
