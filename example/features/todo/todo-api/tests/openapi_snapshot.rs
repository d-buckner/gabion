//! The OpenAPI document is a contract, so it is reviewed like one. The generated
//! JSON is committed at `.gabion/openapi/todo.json`; this test fails when the
//! code and the committed document disagree, which is what makes a schema change
//! show up as a diff instead of as a broken client.
//!
//! Regenerate with:
//! `GABION_BLESS_OPENAPI=1 cargo test -p todo-api --test openapi_snapshot`
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::PathBuf;

fn snapshot_path() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join(".gabion").is_dir() {
            return dir.join(".gabion/openapi/todo.json");
        }
        assert!(dir.pop(), "no .gabion directory above the api crate");
    }
}

fn generated() -> String {
    let mut text = serde_json::to_string_pretty(&todo_api::__todo_server::todo_openapi()).unwrap();
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

/// The snapshot proves nobody changed the code quietly. This proves the generator
/// still connects the trait to the document: every op has a path, every path
/// points at the one error envelope, and the input types are in `components`.
#[test]
fn every_op_is_in_the_document() {
    let doc = todo_api::__todo_server::todo_openapi();
    for path in ["/todo/create", "/todo/complete", "/todo/list"] {
        let op = &doc["paths"][path]["post"];
        assert!(op.is_object(), "{path} is missing from the document");
        assert_eq!(
            op["responses"]["default"]["$ref"], "#/components/responses/Error",
            "{path} does not use the shared error response"
        );
    }
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    for name in ["CreateTodo", "Todo", "TodoId", "WireError"] {
        assert!(schemas.contains_key(name), "{name} has no schema");
    }
}

/// The prose a reviewer reads in the trait is the prose a client integrator
/// reads in the contract, because both come from the same attribute.
#[test]
fn method_docs_reach_the_document() {
    let doc = todo_api::__todo_server::todo_openapi();
    assert_eq!(
        doc["paths"]["/todo/create"]["post"]["summary"],
        "Add one todo. Fails with `EmptyTitle` when the title is blank."
    );
    // `list` has a one-line doc: a summary, and no duplicated description.
    assert!(
        doc["paths"]["/todo/list"]["post"]
            .get("description")
            .is_none(),
        "a one-line doc must not be written twice"
    );
}

/// A `$ref` that points at nothing is a contract a code generator accepts and a
/// client cannot resolve. The ref path comes from schemars' settings, which are
/// set in generated code, so nothing else would catch a wrong prefix.
#[test]
fn every_ref_resolves() {
    let doc = todo_api::__todo_server::todo_openapi();
    let mut refs = Vec::new();
    collect_refs(&doc, &mut refs);
    assert!(refs.len() > 5, "expected refs, found {refs:?}");
    for r in refs {
        let resolved = r
            .strip_prefix("#/components/schemas/")
            .is_some_and(|name| doc["components"]["schemas"].get(name).is_some())
            || r.strip_prefix("#/components/responses/")
                .is_some_and(|name| doc["components"]["responses"].get(name).is_some());
        assert!(resolved, "{r} does not resolve");
    }
}

fn collect_refs(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                if key == "$ref" {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    collect_refs(v, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}
