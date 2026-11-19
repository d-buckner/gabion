//! The incidents OpenAPI document is a contract, so it is reviewed like one. The
//! generated JSON is committed at `.gabion/openapi/incident.json`; this test
//! fails when the code and the committed document disagree.
//!
//! The structural checks a document must pass live in
//! `example/hosts/host-pulse-server/tests/contracts.rs`, which reads every
//! service in this app rather than repeating them per crate.
//!
//! Regenerate with:
//! `GABION_BLESS_OPENAPI=1 cargo test -p incidents-api --test openapi_snapshot`
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::PathBuf;

fn snapshot_path() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join(".gabion").is_dir() {
            return dir.join(".gabion/openapi/incident.json");
        }
        assert!(dir.pop(), "no .gabion directory above the api crate");
    }
}

fn generated() -> String {
    let mut text =
        serde_json::to_string_pretty(&incidents_api::__incident_server::incident_openapi())
            .unwrap();
    text.push('\n');
    text
}

#[test]
fn the_committed_document_matches_the_generated_one() {
    let path = snapshot_path();
    let generated = generated();
    if std::env::var("GABION_BLESS_OPENAPI").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}); regenerate it and commit it",
            path.display()
        )
    });
    assert_eq!(
        committed, generated,
        "the document is stale; regenerate it with GABION_BLESS_OPENAPI=1 and review the diff"
    );
}
