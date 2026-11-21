//! Automatic surface registration: a host mounts an impl, and every surface the
//! contract generates comes with it.
//!
//! Spec §4 planned a `linkme` distributed slice and drew back because "a generic
//! impl per `Caps` bundle makes it non-trivial". The generic impl was the wrong
//! thing to want — a host already holds `Arc<dyn CheckService>`, which is exactly
//! the type registration needs, and `Caps` never has to be named.
//!
//! The first attempt used `linkme` anyway, and it failed in a way worth recording:
//! a distributed slice in an rlib is only collected when the linker happens to
//! pull the object file that contains it. Measured on this toolchain, a real host
//! binary that uses both services collected **nothing** — the registry came back
//! empty and the host would have served no routes while reporting success. That is
//! the fail-open default this project bans (GAPS §1), so registration does not use
//! the linker at all.
//!
//! Instead `#[service]` generates an inherent method *on the trait object*:
//!
//! ```ignore
//! impl dyn CheckService + Send + Sync {
//!     pub fn mount_on(self: Arc<Self>, host: &mut Host) { /* ... */ }
//! }
//! ```
//!
//! So a host writes `checks.mount_on(&mut host)` and names the impl and nothing
//! else: no namespace, no router function, no tool list. It compiles on wasm too,
//! which the linker approach could not.
//!
//! The one thing this cannot see is a contract that no host mounts at all — an
//! absent call cannot be observed at runtime. That is the gate's job:
//! `unserved-contract` fails when a `#[service]` trait is named by no host crate.

use std::any::Any;
use std::sync::Arc;

/// One mounted impl's surfaces. Each is boxed because this module is runtime-free:
/// it cannot name `axum::Router` or `clap::Command`, and the host that owns those
/// types is the one that downcasts them.
pub struct Surfaces {
    /// `todo`.
    pub ns: &'static str,
    /// `TodoService` — what the surface lock records.
    pub trait_name: &'static str,
    /// Every reachable operation, dotted: `todo.create`.
    pub ops: &'static [&'static str],
    /// Every route the `server` feature serves, in trait order.
    pub routes: &'static [&'static str],
    /// `Some` when the api crate was built with the `server` feature.
    pub router: Option<Box<dyn Any + Send>>,
    /// `Some` when built with `cli`.
    pub command: Option<Box<dyn Any + Send>>,
    /// `Some` when built with `mcp`.
    pub mcp: Option<Arc<dyn crate::mcp::Service>>,
    /// `Some` when built with `server` and the contract has a listen op.
    pub ws: Option<Arc<dyn crate::ws::WsService>>,
}

/// A mounted namespace and the MCP view of it, used only to sort before building
/// one server out of several.
type SortedService = (String, Arc<dyn crate::mcp::Service>);

/// The same pair for the stream surface: one connection serves every namespace,
/// so the list is sorted once rather than assembled in mount order.
type SortedStreams = (String, Arc<dyn crate::ws::WsService>);

/// The composition root's view: what it mounted, and what it can serve.
#[derive(Default)]
pub struct Host {
    mounted: Vec<Surfaces>,
}

impl Host {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by the generated `mount_on`. A host does not call it directly; that
    /// is the point — the arguments are everything the host used to have to keep
    /// in step with its api crates by hand.
    pub fn mount(&mut self, surfaces: Surfaces) -> &mut Self {
        self.mounted.push(surfaces);
        self
    }

    /// The namespaces this host serves, in mount order: the reviewable artifact a
    /// host can log at startup and a test can pin.
    #[must_use]
    pub fn mounted(&self) -> Vec<&'static str> {
        self.mounted.iter().map(|m| m.ns).collect()
    }

    /// The contracts this host serves, by trait name — what the surface lock
    /// records, so a test can compare the two.
    #[must_use]
    pub fn contracts(&self) -> Vec<&'static str> {
        self.mounted.iter().map(|m| m.trait_name).collect()
    }

    /// Every reachable operation this host serves, dotted and sorted. A host that
    /// logs this line at startup has said out loud what it exposes.
    #[must_use]
    pub fn exposed(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.mounted.iter().flat_map(|m| m.ops).copied().collect();
        out.sort_unstable();
        out
    }

    /// One boxed router per mounted service that was built with `server`.
    ///
    /// Takes them rather than lending them because a `Box<dyn Any>` cannot be
    /// cloned; a host that wants two apps builds two hosts, which is also the only
    /// way to give them different capability bundles.
    pub fn routers(&mut self) -> Vec<Box<dyn Any + Send>> {
        self.mounted
            .iter_mut()
            .filter_map(|m| m.router.take())
            .collect()
    }

    /// One boxed clap command per mounted service that was built with `cli`.
    pub fn commands(&mut self) -> Vec<Box<dyn Any + Send>> {
        self.mounted
            .iter_mut()
            .filter_map(|m| m.command.take())
            .collect()
    }

    /// Every mounted service the stream surface can serve, sorted by namespace so
    /// the connection does not depend on mount order.
    #[must_use]
    pub fn ws(&self) -> Vec<Arc<dyn crate::ws::WsService>> {
        let mut services: Vec<SortedStreams> = self
            .mounted
            .iter()
            .filter_map(|m| m.ws.clone().map(|s| (s.ns().to_string(), s)))
            .collect();
        services.sort_by(|a, b| a.0.cmp(&b.0));
        services.into_iter().map(|(_, svc)| svc).collect()
    }

    /// Every mounted service as one MCP server, sorted by namespace so the tool
    /// list does not depend on mount order.
    #[must_use]
    pub fn mcp(&self) -> Option<crate::mcp::Server> {
        let mut services: Vec<SortedService> = self
            .mounted
            .iter()
            .filter_map(|m| m.mcp.clone().map(|s| (s.ns().to_string(), s)))
            .collect();
        if services.is_empty() {
            return None;
        }
        services.sort_by(|a, b| a.0.cmp(&b.0));
        let mut server = crate::mcp::Server::new();
        for (_, svc) in services {
            server = server.with(svc);
        }
        Some(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces(
        ns: &'static str,
        trait_name: &'static str,
        ops: &'static [&'static str],
    ) -> Surfaces {
        Surfaces {
            ns,
            trait_name,
            ops,
            routes: ops,
            router: None,
            command: None,
            mcp: None,
            ws: None,
        }
    }

    #[test]
    fn mounting_records_what_the_host_serves() {
        let mut host = Host::new();
        assert_eq!(host.mounted(), Vec::<&str>::new());
        host.mount(surfaces(
            "todo",
            "TodoService",
            &["todo.create", "todo.list"],
        ));
        host.mount(surfaces("check", "CheckService", &["check.add"]));

        assert_eq!(host.mounted(), vec!["todo", "check"]);
        assert_eq!(host.contracts(), vec!["TodoService", "CheckService"]);
        assert_eq!(
            host.exposed(),
            vec!["check.add", "todo.create", "todo.list"],
            "the op list is sorted, so a log line is comparable between runs"
        );
    }

    /// A host with no MCP surface must say `None` rather than hand back an empty
    /// server: an empty server answers `tools/list` with nothing, which reads as a
    /// working endpoint that has no tools.
    #[test]
    fn a_host_without_mcp_has_no_server() {
        let mut host = Host::new();
        host.mount(surfaces("todo", "TodoService", &["todo.create"]));
        assert!(host.mcp().is_none());
    }
}
