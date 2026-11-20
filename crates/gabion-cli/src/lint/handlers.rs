//! `axum-outside-host`: a hand-written HTTP handler.
//!
//! Spec §7: "axum importable only in generated modules + host main — NOT BUILT".
//! The wall it protects is the whole point of generating routes: if a feature can
//! write its own axum handler, the contract is optional, and the OpenAPI document,
//! the MCP tool, the CLI subcommand and the client quietly describe a different
//! service than the one being served.
//!
//! The check is a source scan, because that is where a hand-written handler shows
//! up: `#[service]` emits its axum code as macro expansion, invisible here, so any
//! `axum` in a crate's *source* was written by a person. Hosts are exempt — a host
//! is the composition root and may add an endpoint (a health check,
//! `/openapi.json`) beside the generated ones.
//!
//! Deliberate limit: this reads paths, not types. A crate that reaches axum
//! through a re-export (`use body::Router`) is not caught here; `core-runtime-dep`
//! catches the manifest entry that would make it possible in a core, and only a
//! host may have one.

use std::path::Path;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::UseTree;

use crate::config::Class;
use crate::graph::Crate;
use crate::report::Finding;
use crate::source::line_of;

const RULE: &str = "axum-outside-host";

struct AxumUse {
    hits: Vec<(usize, String)>,
}

/// The first segment of a `use` tree: `axum::Router` -> `axum`.
fn first_segment(tree: &UseTree) -> Option<String> {
    match tree {
        UseTree::Path(p) => Some(p.ident.to_string()),
        UseTree::Group(g) => g.items.iter().find_map(first_segment),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for AxumUse {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        if first_segment(&node.tree).as_deref() == Some("axum") {
            self.hits
                .push((line_of(node.span()), "`use axum::…`".to_string()));
        }
        visit::visit_item_use(self, node);
    }

    /// An inline path, with no `use` at all: `axum::Router::new()`.
    fn visit_path(&mut self, node: &'ast syn::Path) {
        let named = node
            .segments
            .first()
            .is_some_and(|segment| segment.ident == "axum");
        if named {
            self.hits
                .push((line_of(node.span()), "`axum::…` inline".to_string()));
        }
        visit::visit_path(self, node);
    }
}

/// Every app crate that is not a host, and that names axum in its sources.
///
/// Framework crates are exempt: `gabion-native`'s MCP-over-HTTP transport *is* the
/// machinery the surfaces are built from, and the rule is about the app that would
/// otherwise hand-write a handler beside its generated ones.
#[must_use]
pub fn check(krate: &Crate) -> Vec<Finding> {
    if matches!(krate.class, Class::Host | Class::Framework) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for path in krate.source_files() {
        findings.extend(check_file(krate, &path));
    }
    findings
}

fn check_file(krate: &Crate, path: &Path) -> Vec<Finding> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(file) = syn::parse_str::<syn::File>(&text) else {
        return Vec::new();
    };
    let mut collector = AxumUse { hits: Vec::new() };
    visit::visit_file(&mut collector, &file);
    collector
        .hits
        .into_iter()
        .map(|(line, what)| {
            Finding::new(
                RULE,
                krate.at(path, line),
                format!("{what} in a {} crate", krate.class.key()),
                "the HTTP surface is generated from the #[service] trait: a hand-written axum \
                 handler serves a route the OpenAPI document, the MCP tool, the CLI subcommand \
                 and the client do not know about. Move the operation into the trait, or into a \
                 host if it is composition (a health check, /openapi.json).",
            )
        })
        .collect()
}
