//! The public surface is the security boundary.
//!
//! Since every method in a `#[service]` trait is generated into every surface —
//! CLI, HTTP, MCP, typed client — adding a method grants it to every caller,
//! including agents, with no attribute to review. So the reviewable artifact is
//! not an annotation, it is the *set*: this module locks the exact set of
//! reachable operations in `.gabion/surfaces.lock` and fails when it drifts.
//!
//! The point is not the file, it is that the diff shows up in a PR.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{File, ItemTrait, TraitItem};

use crate::config::Class;
use crate::graph::{Crate, Graph};
use crate::report::Finding;

/// Path of the lock file, relative to the workspace root.
pub const LOCK_PATH: &str = ".gabion/surfaces.lock";

/// One `crate::Trait::method` line per reachable operation, plus the names of
/// the traits that produced them.
struct Collected {
    ops: BTreeSet<String>,
    traits: Vec<String>,
}

struct Collector {
    krate: String,
    found: BTreeSet<String>,
    traits: Vec<String>,
}

/// `#[service]`, `#[gabion::service]` and `#[::gabion_macros::service]` all
/// generate the same surfaces, so the collector keys on the last path segment.
/// Keying on the bare spelling let an app publish ops the lock never saw.
fn is_service(path: &syn::Path) -> bool {
    path.segments
        .last()
        .is_some_and(|seg| seg.ident == "service")
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        if node.attrs.iter().any(|a| is_service(a.path())) {
            let trait_name = node.ident.to_string();
            self.traits.push(trait_name.clone());
            for item in &node.items {
                if let TraitItem::Fn(f) = item {
                    self.found
                        .insert(format!("{}::{}::{}", self.krate, trait_name, f.sig.ident));
                }
            }
        }
        syn::visit::visit_item_trait(self, node);
    }
}

fn collect_crate(krate: &Crate) -> Collected {
    let mut v = Collector {
        krate: krate.name.clone(),
        found: BTreeSet::new(),
        traits: Vec::new(),
    };
    for path in krate.source_files() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(file) = syn::parse_str::<File>(&content) {
                syn::visit::visit_file(&mut v, &file);
            }
        }
    }
    Collected {
        ops: v.found,
        traits: v.traits,
    }
}

/// A `#[service]` trait outside `*-api` is a finding: the trait is the boundary,
/// so it must sit in the crate the wall keeps free of implementation.
fn placement_findings(krate: &Crate, collected: &Collected) -> Vec<Finding> {
    if krate.class == Class::Api || collected.traits.is_empty() {
        return Vec::new();
    }
    vec![Finding::new(
        "service-outside-api",
        krate.name.clone(),
        format!(
            "#[service] trait(s) {} declared in a {} crate",
            collected.traits.join(", "),
            krate.class.key()
        ),
        "#[service] generates a route, a subcommand and a client method for every method in the trait, wherever it is written. Declare the trait in the feature's *-api crate.",
    )]
}

fn lock_file(root: &Path) -> PathBuf {
    root.join(LOCK_PATH)
}

fn read_lock(root: &Path) -> Option<BTreeSet<String>> {
    std::fs::read_to_string(lock_file(root)).ok().map(|s| {
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect()
    })
}

fn write_lock(root: &Path, current: &BTreeSet<String>) -> Result<(), String> {
    let path = lock_file(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut body = String::from("# gabion public surface. Every line is reachable over CLI, HTTP,\n# MCP and the typed client. Review changes here like an API change.\n");
    for line in current {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Compare the live surface against the lock. `update` rewrites the lock instead
/// of reporting, which is the "yes, I meant to publish that" gesture.
///
/// Every crate is scanned, not just `*-api`: `#[service]` generates a route and
/// a subcommand wherever it appears, so a trait declared anywhere else is still
/// reachable and still belongs in the lock.
pub fn check(graph: &Graph, root: &Path, update: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current = BTreeSet::new();
    for krate in &graph.crates {
        let collected = collect_crate(krate);
        findings.extend(placement_findings(krate, &collected));
        current.extend(collected.ops);
    }
    if update {
        findings.extend(match write_lock(root, &current) {
            Ok(()) => Vec::new(),
            Err(e) => vec![Finding::new(
                "surface-snapshot",
                LOCK_PATH.to_string(),
                e,
                "make the directory writable or pass a writable workspace root.",
            )],
        });
        return findings;
    }
    let Some(locked) = read_lock(root) else {
        findings.push(Finding::new(
            "surface-snapshot",
            LOCK_PATH.to_string(),
            format!(
                "no surface lock; {} reachable operation(s) are unreviewed",
                current.len()
            ),
            "run `gabion check --update-surfaces` and commit the file. The lock is what makes \
             'this method is now callable by an agent' visible in a diff.",
        ));
        return findings;
    };
    let added: Vec<&String> = current.difference(&locked).collect();
    let removed: Vec<&String> = locked.difference(&current).collect();
    if added.is_empty() && removed.is_empty() {
        return findings;
    }
    let mut detail = Vec::new();
    if !added.is_empty() {
        detail.push(format!("+ reachable: {}", names(&added)));
    }
    if !removed.is_empty() {
        detail.push(format!("- gone: {}", names(&removed)));
    }
    findings.push(Finding::new(
        "surface-snapshot",
        LOCK_PATH.to_string(),
        detail.join("; "),
        "the public surface changed. Every method in a #[service] trait is callable by every \
         caller, including agents, so this is an API change: review it, then run \
         `gabion check --update-surfaces` and commit the lock.",
    ));
    findings
}

fn names(items: &[&String]) -> String {
    items
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
