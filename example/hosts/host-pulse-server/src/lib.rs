//! Server host: two generated routers on one app. The whole crate is wiring —
//! build the caps, build the impls, mount what `#[service]` generated.

use axum::routing::get;
use axum::Json;
use checks_core::CheckServiceImpl;
use incidents_core::IncidentServiceImpl;
use std::sync::Arc;

/// One app, both services. The incidents impl gets its own checks impl because
/// the two features share a store, not an instance.
pub fn app() -> axum::Router {
    let caps = gabion_native::NativeCaps::new();
    let checks: Arc<dyn checks_api::CheckService + Send + Sync> =
        Arc::new(CheckServiceImpl::new(caps.clone()));
    let incidents: Arc<dyn incidents_api::IncidentService + Send + Sync> = Arc::new(
        IncidentServiceImpl::new(caps.clone(), CheckServiceImpl::new(caps)),
    );

    axum::Router::new()
        .merge(checks_api::__check_server::check_router(checks))
        .merge(incidents_api::__incident_server::incident_router(incidents))
        .route("/openapi.json", get(contract))
}

/// The same documents the snapshot tests commit, served from the process that
/// serves the routes they describe.
pub async fn contract() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "checks": checks_api::__check_server::check_openapi(),
        "incidents": incidents_api::__incident_server::incident_openapi(),
    }))
}
