//! The public surface is the security boundary.
//!
//! Since every method in a `#[service]` trait is generated into every surface —
//! CLI, HTTP, MCP, typed client — adding a method grants it to every caller,
//! including agents, with no attribute to review. So the reviewable artifact is
//! not an annotation, it is the *set*: this module locks the exact set of
//! reachable operations in `.gabion/surfaces.lock` and fails when it drifts.
//!
//! The point is not the file, it is that the diff shows up in a PR.
//!
//! # The shape of a line
//!
//! ```text
//! checks-api::CheckService::add #3f2a1b9c
//! ```
//!
//! The names are the reachable set; the hash is the *shape* of the operation — its
//! signature plus the definitions, in the same crate, of the types it names. It
//! exists because a set of names does not notice the change that matters most to a
//! caller: `create(input: CreateTodo)` becoming `create(input: CreateTodoV2)`
//! keeps the name and breaks every client. The OpenAPI snapshot covers that for a
//! service that builds the document; the lock covers it for everyone, including a
//! feature that leaves the `openapi` feature off.
//!
//! The hash has limits, and they are the reason it is a hash and not a proof: it
//! reads the tokens of the signature and the definitions it names, looking them up
//! across the workspace (so `checks_api::CheckId` counts even from another crate),
//! one level deep. Two different types with the same name both feed the hash, which
//! is conservative rather than wrong: the input changes if either does. A change to
//! *how* a type serializes is invisible to it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use quote::ToTokens;

mod placement;
use placement::unserved_findings;
use syn::visit::Visit;
use syn::{File, Item, ItemTrait, TraitItem};

use crate::config::Class;
use crate::graph::{Crate, Graph};
use crate::report::Finding;
use crate::source::mentions;

/// Path of the lock file, relative to the workspace root.
pub const LOCK_PATH: &str = ".gabion/surfaces.lock";

/// One `crate::Trait::method #hash` line per reachable operation, plus the names of
/// the traits that produced them.
struct Collected {
    ops: BTreeSet<String>,
    traits: Vec<String>,
}

struct Collector {
    krate: String,
    found: BTreeSet<String>,
    traits: Vec<String>,
    /// `struct`/`enum`/`type` definitions in the workspace, rendered, so a
    /// signature that names one is hashed with the shape of that definition —
    /// including a type that lives in another crate's contract.
    defs: BTreeMap<String, String>,
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
                    // Tokens, not `Debug`: syn's Debug embeds the span, which is a
                    // byte offset, so the hash would change when the file moves.
                    let signature = f.sig.to_token_stream().to_string();
                    self.found.insert(format!(
                        "{}::{}::{} #{}",
                        self.krate,
                        trait_name,
                        f.sig.ident,
                        digest(&signature, &self.defs)
                    ));
                }
            }
        }
        syn::visit::visit_item_trait(self, node);
    }
}

/// FNV-1a over the signature and every definition it names, as sixteen hex
/// characters. Not a cryptographic hash: it is a drift detector, and a
/// collision means a missed diff, not a security hole.
fn digest(signature: &str, defs: &BTreeMap<String, String>) -> String {
    let mut input = signature.to_string();
    for (name, body) in defs {
        if mentions(signature, name) {
            input.push('\u{1f}');
            input.push_str(body);
        }
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn collect_crate(krate: &Crate, defs: &BTreeMap<String, String>) -> Collected {
    let mut v = Collector {
        krate: krate.name.clone(),
        found: BTreeSet::new(),
        traits: Vec::new(),
        defs: defs.clone(),
    };
    for path in krate.source_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = syn::parse_str::<File>(&text) else {
            continue;
        };
        syn::visit::visit_file(&mut v, &file);
    }
    Collected {
        ops: v.found,
        traits: v.traits,
    }
}

/// Every `struct`/`enum`/`type` definition in the workspace, rendered to tokens
/// and keyed by name.
///
/// Keyed by name because a signature mentions the name it imported, and a
/// definition may live in another crate's contract — which is exactly the case the
/// per-crate version missed. A collision (two crates with a `Todo`) is resolved by
/// including both renderings, sorted, so the hash changes if either type does.
fn workspace_definitions(graph: &Graph) -> BTreeMap<String, String> {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for krate in &graph.crates {
        for path in krate.source_files() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(file) = syn::parse_str::<File>(&text) else {
                continue;
            };
            for item in &file.items {
                let (name, rendered) = match item {
                    Item::Struct(s) => {
                        (s.ident.to_string(), s.fields.to_token_stream().to_string())
                    }
                    Item::Enum(e) => (
                        e.ident.to_string(),
                        e.variants.to_token_stream().to_string(),
                    ),
                    Item::Type(t) => (t.ident.to_string(), t.ty.to_token_stream().to_string()),
                    _ => continue,
                };
                by_name.entry(name).or_default().push(rendered);
            }
        }
    }
    by_name
        .into_iter()
        .map(|(name, mut rendered)| {
            rendered.sort();
            rendered.dedup();
            (name, rendered.join("\u{1e}"))
        })
        .collect()
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
        "#[service] generates a route, a subcommand, a tool and a client method for every method \
         in the trait, wherever it is written. Declare the trait in the feature's *-api crate.",
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
    let mut body = String::from(
        "# gabion public surface. Every line is one reachable operation, hashed over\n\
         # its signature and the definitions it names, in any crate.\n\
         # Reachable over CLI, HTTP, MCP and the typed client. Review changes here like\n\
         # an API change: a changed hash is a changed input or output.\n",
    );
    for line in current {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Compare the live surface against the lock. `update` rewrites the lock instead
/// of reporting, which is the "yes, I meant to publish that" gesture.
///
/// Every crate is scanned, not just `*-api`: `#[service]` generates a route and a
/// subcommand wherever it appears, so a trait declared anywhere else is still
/// reachable and still belongs in the lock.
pub fn check(graph: &Graph, root: &Path, update: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current = BTreeSet::new();
    let mut contracts = BTreeSet::new();
    let defs = workspace_definitions(graph);
    for krate in &graph.crates {
        let collected = collect_crate(krate, &defs);
        findings.extend(placement_findings(krate, &collected));
        for op in &collected.ops {
            let mut parts = op.split("::");
            let (krate_name, trait_name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            contracts.insert(format!("{krate_name}::{trait_name}"));
        }
        current.extend(collected.ops);
    }
    findings.extend(unserved_findings(graph, &contracts));
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
        "the public surface changed — a new operation, a removed one, or a changed input/output \
         type (the hash covers that). Every method in a #[service] trait is callable by every \
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

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should show the hash inputs
mod tests {
    use super::*;

    fn defs() -> BTreeMap<String, String> {
        let mut defs = BTreeMap::new();
        defs.insert("CreateTodo".to_string(), "[title: String]".to_string());
        defs.insert("Unused".to_string(), "[]".to_string());
        defs
    }

    #[test]
    fn a_signature_and_its_definitions_hash_together() {
        let a = digest("create(input: CreateTodo)", &defs());
        let b = digest("create(input: CreateTodo)", &defs());
        assert_eq!(a, b, "the digest is stable");

        let mut changed = defs();
        changed.insert(
            "CreateTodo".to_string(),
            "[title: String, done: bool]".to_string(),
        );
        assert_ne!(
            digest("create(input: CreateTodo)", &changed),
            a,
            "a changed definition must change the hash"
        );
    }

    /// A definition the signature does not name must not leak into the hash: a
    /// feature would otherwise see every one of its operations drift whenever an
    /// unrelated struct changed.
    #[test]
    fn an_unrelated_definition_does_not_change_the_hash() {
        let a = digest("list(ctx: &Ctx)", &defs());
        let mut changed = defs();
        changed.insert("Unused".to_string(), "[field: u8]".to_string());
        assert_eq!(digest("list(ctx: &Ctx)", &changed), a);
    }

    #[test]
    fn a_name_that_is_only_a_prefix_does_not_count_as_a_mention() {
        let mut defs = BTreeMap::new();
        defs.insert("Create".to_string(), "[a: u8]".to_string());
        assert_eq!(
            digest("create(input: CreateTodo)", &defs),
            digest("create(input: CreateTodo)", &BTreeMap::new()),
            "`Create` inside `CreateTodo` is not the same type"
        );
    }
}
