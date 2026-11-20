//! Server host: two generated routers on one app, one MCP endpoint over the same
//! two impls, and the contract documents they describe.
//!
//! The whole crate is wiring — build the caps, build the impls, mount what
//! `#[service]` generated. Nothing here knows what a watch or an incident is.

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
    let checks: Arc<dyn CheckService + Send + Sync> = Arc::new(CheckServiceImpl::new(caps.clone()));
    let incidents: Arc<dyn IncidentService + Send + Sync> = Arc::new(IncidentServiceImpl::new(
        caps.clone(),
        CheckServiceImpl::new(caps),
    ));
    (checks, incidents)
}

/// One app, both services, on every surface the process serves: HTTP routes, the
/// MCP endpoint, and the OpenAPI documents that describe the routes an agent's
/// tool list came from.
pub fn app() -> axum::Router {
    let (checks, incidents) = services();
    let mcp = Arc::new(mcp(checks.clone(), incidents.clone()));

    axum::Router::new()
        .merge(checks_api::__check_server::check_router(checks))
        .merge(incidents_api::__incident_server::incident_router(incidents))
        .merge(gabion_native::mcp_router(mcp))
        .route("/openapi.json", get(contract))
}

/// Both services as tools, over the same impls the routes serve — so an agent and
/// a browser call the same business logic, not two code paths that agree today.
pub fn mcp(checks: Checks, incidents: Incidents) -> gabion::mcp::Server {
    gabion::mcp::Server::new()
        .with(Arc::new(checks_api::__check_mcp::check_mcp(checks)))
        .with(Arc::new(incidents_api::__incident_mcp::incident_mcp(
            incidents,
        )))
}

/// The same documents the snapshot tests commit, served from the process that
/// serves the routes they describe.
pub async fn contract() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "checks": checks_api::__check_server::check_openapi(),
        "incidents": incidents_api::__incident_server::incident_openapi(),
    }))
}
