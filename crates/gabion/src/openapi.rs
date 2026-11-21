//! OpenAPI 3.1 assembly for the routes `#[service]` generates.
//!
//! The document is built from the same types the router deserializes, so a
//! route and its schema cannot drift apart: there is one source, and it is the
//! method signature. `#[service]` calls this; an app calls the generated
//! `<service>_openapi()` and commits the result as a reviewable artifact.
//!
//! One document per service. An app that wants one merged document for all its
//! services merges them itself, because only it knows the order and the titles.

use schemars::Schema;
use serde_json::{json, Map, Value};

/// Assembles one service's document. Paths are added in the order the trait
/// declares them, so the JSON reads top to bottom like the trait does.
pub struct OpenApi {
    title: String,
    version: String,
    tag: String,
    paths: Map<String, Value>,
    streams: Map<String, Value>,
    error: Option<Value>,
}

/// One route in the document, assembled from one trait method.
pub struct Route<'a> {
    pub path: &'a str,
    pub operation_id: &'a str,
    /// First line of the method's doc comment; empty when it has none.
    pub summary: &'a str,
    /// The rest of the doc comment.
    pub description: &'a str,
    /// `None` for a method that takes no input: there is no body to describe.
    pub request: Option<Schema>,
    pub response: Schema,
}

impl OpenApi {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        version: impl Into<String>,
        tag: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            tag: tag.into(),
            paths: Map::new(),
            streams: Map::new(),
            error: None,
        }
    }

    /// The envelope every route returns when the service fails. Routes point at
    /// this one definition rather than repeating it.
    pub fn error_response(&mut self, schema: Schema) {
        self.error = Some(json!({
            "description": "The call failed. `kind` is the machine-readable field.",
            "content": { "application/json": { "schema": schema } },
        }));
    }

    /// One generated route. Empty summary and description are left out rather
    /// than written as `""`.
    pub fn post(&mut self, route: Route<'_>) {
        let Route {
            path,
            operation_id,
            summary,
            description,
            request,
            response,
        } = route;
        let mut operation = Map::new();
        operation.insert("operationId".to_string(), json!(operation_id));
        operation.insert("tags".to_string(), json!([self.tag]));
        if !summary.is_empty() {
            operation.insert("summary".to_string(), json!(summary));
        }
        if !description.is_empty() {
            operation.insert("description".to_string(), json!(description));
        }
        if let Some(request) = request {
            operation.insert(
                "requestBody".to_string(),
                json!({
                    "required": true,
                    "content": { "application/json": { "schema": request } },
                }),
            );
        }
        operation.insert(
            "responses".to_string(),
            json!({
                "200": {
                    "description": "Success.",
                    "content": { "application/json": { "schema": response } },
                },
                "default": { "$ref": "#/components/responses/Error" },
            }),
        );

        let mut item = Map::new();
        item.insert("post".to_string(), Value::Object(operation));
        self.paths.insert(path.to_string(), Value::Object(item));
    }

    /// One listen op. A stream has no route to describe, so it is not a `paths`
    /// entry; it is named here instead, and the committed document still fails its
    /// snapshot test when a stream op drifts.
    pub fn stream(&mut self, op: &str, policy: &str, item: Schema) {
        self.streams
            .insert(op.to_string(), json!({ "policy": policy, "item": item }));
    }

    /// The finished document, with every schema the paths referenced under
    /// `components/schemas`.
    #[must_use]
    pub fn finish(&self, definitions: Map<String, Value>) -> Value {
        let mut components = Map::new();
        components.insert("schemas".to_string(), Value::Object(definitions));
        if let Some(error) = &self.error {
            components.insert("responses".to_string(), json!({ "Error": error }));
        }
        let mut doc = json!({
            "openapi": "3.1.0",
            "info": {
                "title": self.title,
                "version": self.version,
                "x-generator": concat!("gabion ", env!("CARGO_PKG_VERSION")),
            },
            "paths": Value::Object(self.paths.clone()),
            "components": Value::Object(components),
        });
        // Root-level extension, so a validator that does not know it ignores it and
        // a reviewer who does can still diff it. Absent rather than empty: a feature
        // with no listen op must publish the document it published before streams
        // existed, not that document plus a key.
        if !self.streams.is_empty() {
            doc["x-gabion-streams"] = Value::Object(self.streams.clone());
        }
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_schema(name: &str) -> Schema {
        Schema::new_ref(format!("#/components/schemas/{name}"))
    }

    fn null_schema() -> Schema {
        Schema::from(Map::from_iter([("type".to_string(), json!("null"))]))
    }

    fn doc() -> Value {
        let mut api = OpenApi::new("TodoService", "0.1.0", "todo");
        api.error_response(ref_schema("WireError"));
        api.post(Route {
            path: "/api/todo.create",
            operation_id: "todo_create",
            summary: "Create one todo.",
            description: "Create one todo.\nFails if the title is blank.",
            request: Some(ref_schema("CreateTodo")),
            response: ref_schema("Todo"),
        });
        api.post(Route {
            path: "/api/todo.list",
            operation_id: "todo_list",
            summary: "",
            description: "",
            request: None,
            response: null_schema(),
        });
        api.finish(Map::from_iter([(
            "Todo".to_string(),
            json!({ "type": "object" }),
        )]))
    }

    #[test]
    fn a_path_carries_its_schema_and_the_shared_error() {
        let doc = doc();
        let create = &doc["paths"]["/api/todo.create"]["post"];
        assert_eq!(create["operationId"], "todo_create");
        assert_eq!(create["tags"], json!(["todo"]));
        assert_eq!(create["summary"], "Create one todo.");
        assert_eq!(
            create["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/CreateTodo"
        );
        assert_eq!(
            create["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Todo"
        );
        assert_eq!(
            create["responses"]["default"]["$ref"],
            "#/components/responses/Error"
        );
        assert_eq!(doc["components"]["schemas"]["Todo"]["type"], "object");
        assert_eq!(doc["openapi"], "3.1.0");
    }

    /// A method with no input has no body to describe, and an undocumented
    /// method has no prose to invent.
    #[test]
    fn a_method_without_input_or_docs_omits_both() {
        let list = &doc()["paths"]["/api/todo.list"]["post"];
        assert!(list.get("requestBody").is_none(), "{list}");
        assert!(list.get("summary").is_none(), "{list}");
        assert!(list.get("description").is_none(), "{list}");
    }
}
