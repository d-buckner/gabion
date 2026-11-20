//! Whether anything serves a contract.
//!
//! `gabion::registry` closed the wiring a host *does*: mount an impl, get the
//! routes, the subcommand and the tools. What a registry cannot see at runtime is
//! an absent call — a contract nothing mounted is invisible to it — so the gate
//! checks the other direction: every `#[service]` trait must be named by at least
//! one host crate's source. A trait with no host is a published surface nobody can
//! reach, which is the same half-published state a stale lock prevents.
//!
//! Limit: this is a name match, not a proof that the host mounts it. A host that
//! names the trait for another reason would satisfy it; the runtime side is the
//! test in each host, which mounts and asserts on `exposed()`.

use std::collections::BTreeSet;

use crate::config::Class;
use crate::graph::Graph;
use crate::report::Finding;
use crate::source::mentions;

/// `contract` is `crate::Trait` for every published contract in the workspace.
pub(super) fn unserved_findings(graph: &Graph, contracts: &BTreeSet<String>) -> Vec<Finding> {
    let hosts_mention: Vec<String> = graph
        .crates
        .iter()
        .filter(|k| k.class == Class::Host)
        .flat_map(|k| k.source_files_all())
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    contracts
        .iter()
        .map(|contract| {
            let mut parts = contract.split("::");
            let crate_name = parts.next().unwrap_or_default();
            let trait_name = parts.next().unwrap_or_default();
            (contract, crate_name, trait_name)
        })
        .filter(|(_, _, trait_name)| !hosts_mention.iter().any(|text| mentions(text, trait_name)))
        .map(|(_, crate_name, trait_name)| {
            Finding::new(
                "unserved-contract",
                format!("{crate_name}::{trait_name}"),
                format!(
                    "`{trait_name}` is published and no host crate names it, so nothing mounts it"
                ),
                "mount the impl in a host: `svc.mount_on(&mut host)` brings the routes, the \
                 subcommand and the tools from the contract. An operation nobody serves is a \
                 surface a reviewer approved and no caller can reach.",
            )
        })
        .collect()
}
