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
