//! Both pulse contracts, checked structurally. The per-crate snapshot tests
//! prove the committed file matches the generator; this proves the generator
//! still connects a trait to a usable document, for every service in the app.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;

fn documents() -> Vec<(&'static str, Value)> {
    vec![
        ("checks", checks_api::__check_server::check_openapi()),
        (
            "incidents",
            incidents_api::__incident_server::incident_openapi(),
        ),
    ]
}

fn document(wanted: &str) -> Value {
    documents()
        .into_iter()
        .find(|(name, _)| *name == wanted)
        .unwrap()
        .1
}

fn collect_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
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
        Value::Array(items) => {
            for v in items {
                collect_refs(v, out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_op_has_a_path_and_the_shared_error() {
    let expected: [(&str, &[&str]); 2] = [
        (
            "checks",
            &[
                "/api/check.add",
                "/api/check.list",
                "/api/check.probe",
                "/api/check.sweep",
                "/api/check.history",
            ],
        ),
        (
            "incidents",
            &[
                "/api/incident.evaluate",
                "/api/incident.list",
                "/api/incident.uptime",
            ],
        ),
    ];
    for (name, paths) in expected {
        let doc = document(name);
        for path in paths {
            let op = &doc["paths"][path]["post"];
            assert!(
                op.is_object(),
                "{name}: {path} is missing from the document"
            );
            assert_eq!(
                op["responses"]["default"]["$ref"], "#/components/responses/Error",
                "{name}: {path} does not use the shared error response"
            );
        }
    }
}

/// Every type a method takes or returns is in `components`, so a code generator
/// can build a caller from the document alone. The feature's own error enum is
/// not there on purpose: failures travel as the shared `WireError` envelope, and
/// a client that switches on a per-feature detail enum breaks the moment the
/// feature adds a variant.
#[test]
fn every_input_and_output_type_has_a_schema() {
    let expected: [(&str, &[&str]); 2] = [
        (
            "checks",
            &[
                "NewCheck",
                "Check",
                "CheckId",
                "Probe",
                "CheckReport",
                "WireError",
                "ErrorKind",
            ],
        ),
        (
            "incidents",
            &[
                // `CheckId` is the checks type, and incidents names it: one
                // contract may take another contract's id.
                "CheckId",
                "Incident",
                "Uptime",
                "WireError",
                "ErrorKind",
            ],
        ),
    ];
    for (name, types) in expected {
        let doc = document(name);
        for type_name in types {
            assert!(
                doc["components"]["schemas"].get(type_name).is_some(),
                "{name}: {type_name} has no schema"
            );
        }
    }
}

/// A `$ref` that points at nothing is a contract a code generator accepts and a
/// client cannot resolve.
#[test]
fn every_ref_resolves() {
    for (name, doc) in documents() {
        let mut refs = Vec::new();
        collect_refs(&doc, &mut refs);
        assert!(refs.len() > 5, "{name}: expected refs, found {refs:?}");
        for r in refs {
            let resolved = r
                .strip_prefix("#/components/schemas/")
                .is_some_and(|n| doc["components"]["schemas"].get(n).is_some())
                || r.strip_prefix("#/components/responses/")
                    .is_some_and(|n| doc["components"]["responses"].get(n).is_some());
            assert!(resolved, "{name}: {r} does not resolve");
        }
    }
}

/// The prose a reviewer reads in the trait is the prose a client integrator
/// reads in the contract.
#[test]
fn method_docs_reach_the_document() {
    let doc = document("checks");
    assert_eq!(
        doc["paths"]["/api/check.add"]["post"]["summary"],
        "Start watching a URL. Fails with `InvalidUrl` unless it is http or https."
    );
    assert!(
        doc["paths"]["/api/check.list"]["post"]
            .get("description")
            .is_none(),
        "a one-line doc must not be written twice"
    );
}
