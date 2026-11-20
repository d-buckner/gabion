//! The gabion-specific source rules. Each one is a `syn` pass that produces
//! findings; the dispatcher in `super` decides which crates each rule applies to.

use std::path::Path;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, File};

use crate::config::Class;
use crate::graph::Crate;
use crate::report::Finding;
use crate::source::{has_cfg_test, line_of, nested_paths, span_range, Comment, Regions};

// --- Dune's comment rule: no comments inside *-core fn bodies --------------

struct BodyCollector {
    bodies: Vec<(usize, usize)>,
    skip: Vec<(usize, usize)>,
}

impl<'ast> Visit<'ast> for BodyCollector {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.bodies
            .push(span_range(node.block.brace_token.span.join()));
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.bodies
            .push(span_range(node.block.brace_token.span.join()));
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if has_cfg_test(&node.attrs) {
            if let Some((brace, _)) = &node.content {
                self.skip.push(span_range(brace.span.join()));
            }
        }
        syn::visit::visit_item_mod(self, node);
    }
}

pub(crate) fn comment_rule(
    krate: &Crate,
    path: &Path,
    file: &File,
    comments: &[Comment],
    regions: &Regions,
) -> Vec<Finding> {
    if krate.class != Class::Core {
        return Vec::new();
    }
    let mut v = BodyCollector {
        bodies: Vec::new(),
        skip: regions.test.clone(),
    };
    visit::visit_file(&mut v, file);
    comments
        .iter()
        .filter(|c| !c.doc)
        .filter(|c| v.bodies.iter().any(|(a, b)| c.line >= *a && c.line <= *b))
        .filter(|c| !v.skip.iter().any(|(a, b)| c.line >= *a && c.line <= *b))
        .map(|c| {
            Finding::new(
                "no-comments-in-service-bodies",
                krate.at(path, c.line),
                c.text.trim().to_string(),
                "business logic must read as prose: name the concept the comment explains and extract it into a named fn or const. Doc comments on items are still required.",
            )
        })
        .collect()
}

// --- silent data loss in the contract layer --------------------------------

struct SilentDefault {
    hits: Vec<usize>,
}

impl<'ast> Visit<'ast> for SilentDefault {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "unwrap_or_default" {
            self.hits.push(line_of(node.method.span()));
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

pub(crate) fn silent_default_rule(krate: &Crate, path: &Path, file: &File) -> Vec<Finding> {
    if !matches!(krate.class, Class::Api | Class::Core) {
        return Vec::new();
    }
    let mut v = SilentDefault { hits: Vec::new() };
    visit::visit_file(&mut v, file);
    v.hits
        .into_iter()
        .map(|line| {
            Finding::new(
                "core-silent-default",
                krate.at(path, line),
                ".unwrap_or_default() in the contract layer".to_string(),
                "this is how a serialization failure becomes an empty string that later parses to nothing: silent data loss. Propagate the error — clippy's unwrap_used ban does not catch this one.",
            )
        })
        .collect()
}

// --- hosts are composition only --------------------------------------------

struct HostLogic {
    hits: Vec<(usize, String)>,
}

impl<'ast> Visit<'ast> for HostLogic {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if let Some((_, path, _)) = &node.trait_ {
            if let Some(last) = path.segments.last() {
                let name = last.ident.to_string();
                if name.ends_with("Service") {
                    self.hits.push((line_of(node.self_ty.span()), name));
                }
            }
        }
        syn::visit::visit_item_impl(self, node);
    }
}

pub(crate) fn host_logic_rule(krate: &Crate, path: &Path, file: &File) -> Vec<Finding> {
    if krate.class != Class::Host {
        return Vec::new();
    }
    let mut v = HostLogic { hits: Vec::new() };
    visit::visit_file(&mut v, file);
    v.hits
        .into_iter()
        .map(|(line, trait_name)| {
            Finding::new(
                "host-business-logic",
                krate.at(path, line),
                format!("impl {trait_name} for ... lives in a host crate"),
                "hosts wire things together; business logic belongs in *-core, where it is testable and reaches every surface.",
            )
        })
        .collect()
}

// --- contract crates name what they publish --------------------------------

struct GlobReexport {
    hits: Vec<(usize, String)>,
}

impl<'ast> Visit<'ast> for GlobReexport {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        if matches!(node.vis, syn::Visibility::Public(_)) {
            let rendered = render_use_tree(&node.tree);
            if rendered.contains('*') {
                self.hits.push((line_of(node.vis.span()), rendered));
            }
        }
        syn::visit::visit_item_use(self, node);
    }
}

fn render_use_tree(tree: &syn::UseTree) -> String {
    use syn::UseTree;
    match tree {
        UseTree::Path(p) => format!("{}::{}", p.ident, render_use_tree(&p.tree)),
        UseTree::Name(n) => n.ident.to_string(),
        UseTree::Rename(r) => format!("{} as {}", r.ident, r.rename),
        UseTree::Glob(_) => "*".to_string(),
        UseTree::Group(g) => {
            let items: Vec<String> = g.items.iter().map(render_use_tree).collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// A `pub use module::*` in a contract crate publishes whatever that module
/// exports today and whatever it grows later. clippy's `wildcard_imports` does
/// not lint `pub use` globs (measured on rust 1.99), so this is the only thing
/// that sees it.
pub(crate) fn contract_glob_rule(krate: &Crate, path: &Path, file: &File) -> Vec<Finding> {
    if krate.class != Class::Api {
        return Vec::new();
    }
    let mut v = GlobReexport { hits: Vec::new() };
    visit::visit_file(&mut v, file);
    v.hits
        .into_iter()
        .map(|(line, rendered)| {
            Finding::new(
                "contract-glob",
                krate.at(path, line),
                format!("`pub use {rendered}` publishes a whole module"),
                "the contract is the list of names an outside caller may use; name them. A glob hands the wall to whatever the target module exports next.",
            )
        })
        .collect()
}

// --- #[allow] placement audit ----------------------------------------------

/// Lints gabion denies on purpose. Suppressing one is only legal where a rule
/// grants it — otherwise a one-line `#[allow]` deletes a wall.
const MANAGED: [&str; 7] = [
    "clippy::disallowed_types",
    "clippy::disallowed_methods",
    "clippy::unwrap_used",
    "clippy::expect_used",
    "clippy::all",
    "clippy::pedantic",
    "missing_docs",
];

struct AllowCollector {
    hits: Vec<(usize, Vec<String>)>,
}

impl<'ast> Visit<'ast> for AllowCollector {
    fn visit_attribute(&mut self, node: &'ast Attribute) {
        if node.path().is_ident("allow") || node.path().is_ident("expect") {
            let names = nested_paths(node);
            if names.iter().any(|n| MANAGED.contains(&n.as_str())) {
                self.hits.push((line_of(node.pound_token.span), names));
            }
        }
        syn::visit::visit_attribute(self, node);
    }
}

pub(crate) fn allow_audit(
    krate: &Crate,
    path: &Path,
    file: &File,
    regions: &Regions,
) -> Vec<Finding> {
    let mut v = AllowCollector { hits: Vec::new() };
    visit::visit_file(&mut v, file);
    let mut out = Vec::new();
    for (line, names) in v.hits {
        for name in names.iter().filter(|n| MANAGED.contains(&n.as_str())) {
            if granted(krate, name, line, regions) {
                continue;
            }
            let rule = if name.starts_with("clippy::disallowed_") {
                "effect-home-allow"
            } else {
                "allow-allowlist"
            };
            out.push(Finding::new(
                rule,
                krate.at(path, line),
                format!("#[allow({name})] in `{}`", krate.name),
                suppression_fix(name),
            ));
        }
    }
    out
}

fn granted(krate: &Crate, name: &str, line: usize, regions: &Regions) -> bool {
    // An integration test is an effect home by construction: it runs on the
    // developer's machine and ships nowhere, so a test that writes a fixture file
    // is doing the thing tests do. The grant is scoped to the file, and the rule
    // still audits every shipped source.
    let in_test_region = regions.covers(&regions.test, line);
    match name {
        "clippy::disallowed_types" | "clippy::disallowed_methods" => {
            krate.is_effect_home || in_test_region
        }
        "clippy::unwrap_used" | "clippy::expect_used" => krate.is_effect_home || in_test_region,
        "clippy::all" | "clippy::pedantic" | "missing_docs" => {
            regions.covers(&regions.generated, line)
        }
        _ => false,
    }
}

fn suppression_fix(name: &str) -> &'static str {
    if name.starts_with("clippy::disallowed_") {
        "the effect ban is global on purpose. Only a declared effect home (*-native, a host, a test region, or [workspace.metadata.gabion] effect-homes) may lift it."
    } else if name == "clippy::unwrap_used" || name == "clippy::expect_used" {
        "unwrap is allowed in hosts, effect homes, and #[cfg(test)] modules only. Return the error, or move the code to a host."
    } else {
        "broad allows are only legal inside generated modules (a mod named __* emitted by #[service])."
    }
}
