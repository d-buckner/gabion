//! The checks contract's MCP surface, driven the way an agent drives it: list the
//! tools, then call one. The todo contract's `mcp.rs` covers the protocol cases
//! in depth; this is the second service, whose ops take a neighbour-shaped id.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use checks_api::{Check, CheckErr, CheckId, CheckService, NewCheck, Probe};
use gabion::prelude::{async_trait, Ctx};

struct Stub;

#[async_trait]
impl CheckService for Stub {
    async fn add(&self, _ctx: &Ctx, input: NewCheck) -> Result<Check, CheckErr> {
        Ok(Check {
            id: "c1".into(),
            name: input.name,
            url: input.url,
            interval_secs: 60,
            timeout_ms: 2_000,
            created_at: 1,
        })
    }

    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<checks_api::CheckReport>, CheckErr> {
        Ok(Vec::new())
    }

    async fn probe(&self, _ctx: &Ctx, _input: CheckId) -> Result<Probe, CheckErr> {
        unreachable!("this test only calls add and list")
    }

    async fn sweep(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<Probe>, CheckErr> {
        unreachable!("this test only calls add and list")
    }

    async fn history(&self, _ctx: &Ctx, _input: CheckId) -> Result<Vec<Probe>, CheckErr> {
        unreachable!("this test only calls add and list")
    }
}

async fn ask(request: gabion::serde_json::Value) -> gabion::serde_json::Value {
    let svc: Arc<dyn CheckService + Send + Sync> = Arc::new(Stub);
    let server = gabion::mcp::Server::new().with(Arc::new(checks_api::__check_mcp::check_mcp(svc)));
    let reply = server.handle(&request.to_string()).await.unwrap();
    gabion::serde_json::from_str(&reply).unwrap()
}

#[tokio::test]
async fn every_op_in_the_trait_is_a_tool() {
    let reply = ask(gabion::serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"tools/list"
    }))
    .await;
    let tools = reply["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "check.add",
            "check.history",
            "check.list",
            "check.probe",
            "check.sweep"
        ]
    );
}

#[tokio::test]
async fn an_op_that_takes_an_input_publishes_its_schema() {
    let reply = ask(gabion::serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"check.add","arguments":{"name":"api","url":"https://example.com"}}
    }))
    .await;
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let added: gabion::serde_json::Value = gabion::serde_json::from_str(text).unwrap();
    assert_eq!(added["name"], "api");
}
