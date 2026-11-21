//! The incidents contract's MCP surface. This one takes `CheckId` from another
//! contract, so the tool schema and the argument parse are exercising a
//! neighbour's wire type.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use checks_api::{CheckErr, CheckId, CheckReport, CheckService, NewCheck, Probe};
use gabion::prelude::{service_impl, Ctx, Each, Latest};
use incidents_api::{IncidentErr, IncidentService, Uptime};

struct Stub;

#[service_impl]
impl CheckService for Stub {
    async fn add(&self, _ctx: &Ctx, _input: NewCheck) -> Result<checks_api::Check, CheckErr> {
        unreachable!("incidents never adds a watch")
    }
    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<CheckReport>, CheckErr> {
        Ok(Vec::new())
    }
    async fn probe(&self, _ctx: &Ctx, _input: CheckId) -> Result<Probe, CheckErr> {
        unreachable!("incidents never probes")
    }
    async fn sweep(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<Probe>, CheckErr> {
        unreachable!("incidents never sweeps")
    }
    async fn history(&self, _ctx: &Ctx, _input: CheckId) -> Result<Vec<Probe>, CheckErr> {
        Ok(Vec::new())
    }

    async fn watch(&self, _ctx: &Ctx, _input: CheckId) -> Result<Each<Probe>, CheckErr> {
        Ok(Each::new(gabion::stream::from_vec(Vec::new())))
    }

    async fn tail(&self, _ctx: &Ctx, _input: CheckId) -> Result<Latest<Probe>, CheckErr> {
        Ok(Latest::new(gabion::stream::from_vec(Vec::new())))
    }
}

#[service_impl]
impl IncidentService for Stub {
    async fn evaluate(
        &self,
        _ctx: &Ctx,
        _input: (),
    ) -> Result<Vec<incidents_api::Incident>, IncidentErr> {
        Ok(Vec::new())
    }
    async fn list(
        &self,
        _ctx: &Ctx,
        _input: (),
    ) -> Result<Vec<incidents_api::Incident>, IncidentErr> {
        Ok(Vec::new())
    }
    async fn uptime(&self, _ctx: &Ctx, input: CheckId) -> Result<Uptime, IncidentErr> {
        Ok(Uptime {
            check_id: input.0,
            probes: 2,
            ok: 1,
            pct: 50,
        })
    }
}

async fn ask(request: gabion::serde_json::Value) -> gabion::serde_json::Value {
    let svc: Arc<dyn IncidentService + Send + Sync> = Arc::new(Stub);
    let server =
        gabion::mcp::Server::new().with(Arc::new(incidents_api::__incident_mcp::incident_mcp(svc)));
    let reply = server.handle(&request.to_string()).await.unwrap();
    gabion::serde_json::from_str(&reply).unwrap()
}

#[tokio::test]
async fn a_neighbour_contract_id_is_the_tool_argument() {
    let reply = ask(gabion::serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"incident.uptime","arguments":"c1"}
    }))
    .await;
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let uptime: gabion::serde_json::Value = gabion::serde_json::from_str(text).unwrap();
    assert_eq!(uptime["check_id"], "c1");
    assert_eq!(uptime["pct"], 50);
}
