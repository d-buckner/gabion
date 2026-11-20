//! The file store's one promise — what was written is what is read — and the
//! cache that makes a board with hundreds of watches load without re-parsing
//! the whole file once per row.
//!
//! Tests are effect homes: they touch the filesystem and unwrap freely.

#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use gabion::caps::Store;
use gabion_native::FileStore;

#[tokio::test]
async fn writes_are_visible_through_every_clone() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("s.json")).unwrap();
    let reader = store.clone();

    store.set("check/a", r#"{"id":"a"}"#).await.unwrap();

    assert_eq!(
        reader.get("check/a").await.unwrap(),
        Some(r#"{"id":"a"}"#.to_string())
    );
    assert_eq!(
        reader.list("check/").await.unwrap(),
        vec![("check/a".to_string(), r#"{"id":"a"}"#.to_string())]
    );
}

#[tokio::test]
async fn a_fresh_store_reads_what_the_last_one_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.json");

    {
        let first = FileStore::new(&path).unwrap();
        first.set("incident/i1", "{}").await.unwrap();
        assert!(first.get("incident/i1").await.unwrap().is_some());
    }

    let second = FileStore::new(&path).unwrap();
    assert_eq!(
        second.get("incident/i1").await.unwrap(),
        Some("{}".to_string()),
        "a restart must see the file, not an empty cache"
    );
}

#[tokio::test]
async fn a_corrupt_file_is_an_error_not_an_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.json");
    std::fs::write(&path, "{not json").unwrap();

    let store = FileStore::new(&path).unwrap();
    assert!(
        store.list("").await.is_err(),
        "a corrupt store must fail loudly; an empty result would drop every watch"
    );
}

/// Attribution survives a restart: the sidecar is written with the map, so "who
/// wrote this" is answerable after the process that wrote it is gone. That is the
/// difference between a store that records a caller and one that remembers it for
/// as long as it happens to be open.
#[test]
fn a_writer_is_recorded_and_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    {
        let store = gabion_native::FileStore::new(&path).unwrap();
        let ctx = gabion::prelude::Ctx::anonymous().with_principal("agent-7");
        runtime
            .block_on(store.set_as(&ctx, "todo/t1", "{\"title\":\"milk\"}"))
            .unwrap();
        assert_eq!(
            store.written_by("todo/t1"),
            Some("agent-7".to_string()),
            "the writer is recorded beside the value"
        );
        // The app's own data keeps its shape: the sidecar is a separate file.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("agent-7"),
            "the caller leaked into the map: {raw}"
        );
    }

    let reopened = gabion_native::FileStore::new(&path).unwrap();
    assert_eq!(
        reopened.written_by("todo/t1"),
        Some("agent-7".to_string()),
        "attribution must survive the process that wrote it"
    );
}

/// A plain `set` records nothing, and deleting a key drops its writer: a stale
/// attribution attached to a re-created row would be worse than none.
#[test]
fn an_unattributed_write_and_a_delete_leave_no_writer_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let store = gabion_native::FileStore::new(&path).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ctx = gabion::prelude::Ctx::anonymous().with_principal("agent-7");

    runtime.block_on(store.set("k", "v")).unwrap();
    assert_eq!(store.written_by("k"), None);

    runtime.block_on(store.set_as(&ctx, "k", "v")).unwrap();
    assert_eq!(store.written_by("k"), Some("agent-7".to_string()));

    runtime.block_on(store.delete("k")).unwrap();
    assert_eq!(store.written_by("k"), None);
}
