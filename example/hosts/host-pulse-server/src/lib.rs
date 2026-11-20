//! Server host: two generated routers on one app, one MCP endpoint over the same
//! two impls, and the contract documents they describe.
//!
//! The whole crate is wiring — build the caps, choose the impls, mount them.
//! Nothing here knows what a watch or an incident is, and nothing here names a
//! route: `mount_on` brings the routes, the tools and the op list from the
//! contract.

use axum::routing::get;
use axum::Json;
use checks_api::CheckService;
use checks_core::CheckServiceImpl;
use incidents_api::IncidentService;
use incidents_core::IncidentServiceImpl;
use std::sync::Arc;

/// The trait object a host holds for the checks contract.
pub type Checks = Arc<dyn CheckService + Send + Sync>;
/// The trait object a host holds for the incidents contract.
pub type Incidents = Arc<dyn IncidentService + Send + Sync>;

/// Both service impls over one capability bundle. The incidents impl gets its own
/// checks impl because the two features share a store, not an instance.
pub fn services() -> (Checks, Incidents) {
    let caps = gabion_native::NativeCaps::new();
    let checks: Checks = Arc::new(CheckServiceImpl::new(caps.clone()));
    let incidents: Incidents = Arc::new(IncidentServiceImpl::new(
        caps.clone(),
        CheckServiceImpl::new(caps),
    ));
    (checks, incidents)
}

/// Everything this host serves, from the two lines that choose the impls.
pub fn host() -> gabion::registry::Host {
    let (checks, incidents) = services();
    let mut host = gabion::registry::Host::new();
    checks.mount_on(&mut host);
    incidents.mount_on(&mut host);
    host
}

/// One app, both services, on every surface the process serves: the generated
/// routes, the MCP endpoint, and the OpenAPI documents that describe the routes
/// an agent's tool list came from.
pub fn app() -> axum::Router {
    gabion_native::host_router(&mut host()).route("/openapi.json", get(contract))
}

/// The same services as one MCP server, for the stdio transport.
///
/// # Panics
/// Only if the api crates were built without `mcp`, which this crate's manifest
/// requires — a host that mounts a contract and then has no tools is a build
/// error here, not a runtime surprise: see `tests/registry.rs`.
pub fn mcp() -> gabion::mcp::Server {
    match host().mcp() {
        Some(server) => server,
        None => panic!("this host requires the api crates' `mcp` feature"),
    }
}

/// The same documents the snapshot tests commit, served from the process that
/// serves the routes they describe.
pub async fn contract() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "checks": checks_api::__check_server::check_openapi(),
        "incidents": incidents_api::__incident_server::incident_openapi(),
    }))
}
