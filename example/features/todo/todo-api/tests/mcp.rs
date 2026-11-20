//! The generated MCP surface, driven through the protocol rather than through
//! the generator: `initialize`, `tools/list`, `tools/call`.
//!
//! The interesting assertions are the ones a hand-written MCP server gets wrong:
//! the tool list is the *trait* (so an op added there appears here), the
//! description is the doc comment a reviewer read, a bad argument is
//! `invalid_input` rather than a call with defaults, and the caller in `_meta`
//! reaches the impl.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use gabion::prelude::{async_trait, Ctx};
use gabion::serde_json::{json, Value};
use todo_api::{Todo, TodoErr, TodoId, TodoService};

/// A stub that records the caller and answers whatever it was asked, so the test
/// observes the surface's work instead of a store's.
/// One recorded call: the op, and the principal the surface delivered.
type Recorded = (String, Option<String>);

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Recorded>>>);

impl Seen {
    fn last(&self) -> (String, Option<String>) {
        self.0.lock().unwrap().last().cloned().expect("no call")
    }
}

struct Stub(Seen);

#[async_trait]
impl TodoService for Stub {
    async fn create(&self, ctx: &Ctx, input: todo_api::CreateTodo) -> Result<Todo, TodoErr> {
        self.0
             .0
            .lock()
            .unwrap()
            .push(("create".into(), ctx.principal().map(String::from)));
        Ok(Todo {
            id: "t1".into(),
            title: input.title,
            done: false,
            created_at: 7,
        })
    }

    async fn complete(&self, ctx: &Ctx, input: TodoId) -> Result<Todo, TodoErr> {
        self.0
             .0
            .lock()
            .unwrap()
            .push(("complete".into(), ctx.principal().map(String::from)));
        if input.0 == "missing" {
            return Err(todo_api::todo_err(todo_api::TodoError::NotFound));
        }
        Ok(Todo {
            id: input.0,
            title: "milk".into(),
            done: true,
            created_at: 7,
        })
    }

    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr> {
        self.0
             .0
            .lock()
            .unwrap()
            .push(("list".into(), ctx.principal().map(String::from)));
        Ok(Vec::new())
    }
}

fn server() -> (gabion::mcp::Server, Seen) {
    let seen = Seen::default();
    let svc: Arc<dyn TodoService + Send + Sync> = Arc::new(Stub(seen.clone()));
    let mcp = gabion::mcp::Server::new().with(Arc::new(todo_api::__todo_mcp::todo_mcp(svc)));
    (mcp, seen)
}

async fn ask(request: Value) -> Value {
    let (server, _) = server();
    let reply = server.handle(&request.to_string()).await.unwrap();
    gabion::serde_json::from_str(&reply).unwrap()
}

#[tokio::test]
async fn the_tool_list_is_the_trait() {
    let reply = ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await;
    let tools = reply["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["todo.complete", "todo.create", "todo.list"]);

    let create = tools.iter().find(|t| t["name"] == "todo.create").unwrap();
    assert_eq!(
        create["description"],
        "Add one todo. Fails with `EmptyTitle` when the title is blank."
    );
    assert_eq!(
        create["inputSchema"]["properties"]["title"]["type"], "string",
        "the schema is built from the type the route deserializes: {create}"
    );

    let list = tools.iter().find(|t| t["name"] == "todo.list").unwrap();
    assert_eq!(
        list["inputSchema"]["additionalProperties"], false,
        "a tool that takes nothing must say so: {list}"
    );
}

#[tokio::test]
async fn a_tool_call_reaches_the_impl_and_answers_with_its_value() {
    let (server, seen) = server();
    let reply = server
        .handle(
            &json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"todo.create","arguments":{"title":"milk"}}
            })
            .to_string(),
        )
        .await
        .unwrap();
    let reply: Value = gabion::serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["result"]["isError"], false);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let created: Value = gabion::serde_json::from_str(text).unwrap();
    assert_eq!(created["title"], "milk");
    assert_eq!(seen.last().0, "create");
}

/// The caller travels in `params._meta`, which is where the MCP wire format puts
/// per-request metadata — so an agent's calls are identified calls.
#[tokio::test]
async fn the_caller_travels_in_meta() {
    let (server, seen) = server();
    server
        .handle(
            &json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{
                    "name":"todo.list",
                    "arguments":{},
                    "_meta":{"principal":"agent-7","tenant":"acme"}
                }
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert_eq!(seen.last().1.as_deref(), Some("agent-7"));
}

/// A failure keeps the kind that is a 404 over HTTP and exit 3 in the CLI.
#[tokio::test]
async fn a_failed_call_carries_the_kind() {
    let reply = ask(json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"todo.complete","arguments":"missing"}
    }))
    .await;
    assert_eq!(
        reply["error"]["code"],
        gabion::prelude::ErrorKind::NotFound.mcp_code()
    );
    assert_eq!(reply["error"]["data"]["kind"], "not_found");
    assert_eq!(reply["error"]["data"]["detail"], "NotFound");
}

/// Arguments that do not match the declared input type are `invalid_input`,
/// naming the tool. A server that filled in defaults would run a call the caller
/// did not ask for.
#[tokio::test]
async fn bad_arguments_are_invalid_input_not_a_default_call() {
    let reply = ask(json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params":{"name":"todo.create","arguments":{"title": 7}}
    }))
    .await;
    assert_eq!(
        reply["error"]["code"],
        gabion::prelude::ErrorKind::InvalidInput.mcp_code()
    );
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bad arguments for todo.create"),
        "got: {reply}"
    );
}
