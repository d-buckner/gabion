//! The composition root, from the registry's point of view: two lines choose two
//! impls, and everything the contracts generate is mounted.
//!
//! A test rather than a comment because this is where a host's wiring used to
//! drift: a router merged here, a subcommand forgotten there, a tool list that
//! only the CLI knew about. `exposed()` is the artifact to pin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use checks_api::CheckService;
use incidents_api::IncidentService;
use std::sync::Arc;

#[test]
fn the_host_mounts_both_contracts_by_naming_only_the_impls() {
    let mut host = host_pulse_server::host();
    assert_eq!(host.mounted(), vec!["check", "incident"]);
    assert_eq!(host.contracts(), vec!["CheckService", "IncidentService"]);
    assert_eq!(
        host.exposed(),
        vec![
            "check.add",
            "check.history",
            "check.list",
            "check.probe",
            "check.sweep",
            "check.tail",
            "check.watch",
            "incident.evaluate",
            "incident.list",
            "incident.uptime",
        ]
    );
    assert_eq!(host.routers().len(), 2, "one generated router each");
    // Only checks streams, so only checks contributes a stream surface. A feature
    // with no listen op must not gain a `/api/ws` handler it cannot serve.
    let mounted = host.ws();
    let streams: Vec<&str> = mounted.iter().map(|s| s.ns()).collect();
    assert_eq!(streams, vec!["check"]);
}

/// The MCP server the stdio transport serves is the same mounted set, so an
/// agent and a browser see one surface set, not two that agree today.
#[test]
fn the_mcp_server_is_the_mounted_set() {
    let server = host_pulse_server::mcp();
    let names: Vec<String> = server.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
            "check.add",
            "check.history",
            "check.list",
            "check.probe",
            "check.sweep",
            "check.tail",
            "check.watch",
            "incident.evaluate",
            "incident.list",
            "incident.uptime",
        ]
    );
}

/// Mounting the same contract twice is a host bug, not a framework one, but the
/// registry reports it rather than silently serving the second impl: two routers
/// for one namespace would collide on every route at merge time.
#[test]
fn mounting_one_contract_twice_is_visible() {
    let mut host = gabion::registry::Host::new();
    let checks: Arc<dyn CheckService + Send + Sync> = Arc::new(checks_core::CheckServiceImpl::new(
        gabion_native::NativeCaps::new(),
    ));
    checks.clone().mount_on(&mut host);
    checks.mount_on(&mut host);
    assert_eq!(host.mounted(), vec!["check", "check"]);
    // `mcp()` sorts by namespace and keeps both, so the duplicate is not hidden.
    assert_eq!(host.mcp().unwrap().tools().len(), 14);
}

/// And the incidents contract really is independent of checks here: this host
/// mounts it with an impl that was given a different checks instance, which the
/// registry does not care about — it mounts surfaces, not wiring.
#[test]
fn a_contract_mounts_without_the_host_knowing_its_ops() {
    let mut host = gabion::registry::Host::new();
    let caps = gabion_native::NativeCaps::new();
    let checks = checks_core::CheckServiceImpl::new(caps.clone());
    let incidents = incidents_core::IncidentServiceImpl::new(caps.clone(), checks);
    let svc: Arc<dyn IncidentService + Send + Sync> = Arc::new(incidents);
    svc.mount_on(&mut host);
    assert_eq!(host.mounted(), vec!["incident"]);
    assert_eq!(host.exposed().len(), 3);
}
