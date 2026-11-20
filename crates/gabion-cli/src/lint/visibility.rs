//! `over-broad-visibility`: `pub` past the wall.
//!
//! Spec §7 listed a cargo-mend-style visibility check and never built it, so a
//! `pub` item in a `*-core` crate was published to every crate in the workspace
//! with nothing to review. The edge table stops another *feature* from depending
//! on the core, but `pub` is still the difference between "an implementation
//! detail" and "something a host is allowed to name", and nothing said which was
//! which.
//!
//! The mechanical version, chosen because it is decidable: **a `pub` item in a
//! `*-core` crate that no other crate in the workspace names is over-broad** — it
//! should be `pub(crate)`. That is not a proof that the remaining `pub` items are
//! *needed*; it is the one direction that is checkable, and it closes the failure
//! that matters: internals becoming API by default.
//!
//! It applies to `*-render` for the same reason it applies to `*-core`, and that
//! is what makes `render -> render` safe: a UI crate's `pub` items are its
//! components (other features may compose them), and everything else is
//! `pub(crate)`. Without this rule the new edge would mean "reach into another
//! feature's private sub-components", which is the wall `render-isolation` used to
//! buy by banning the edge outright.
//!
//! Limits, stated rather than hidden: the check reads names, not types, so a name
//! mentioned in a comment counts as a use; it looks at top-level items, not
//! associated functions or re-exports; and it does not apply to `*-api`, because a
//! contract's whole purpose is to publish names — the surface lock is what governs
//! which of them are reachable.

use std::collections::BTreeSet;

use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::Visibility;
use syn::{File, Item};

use crate::config::Class;
use crate::graph::{Crate, Graph};
use crate::report::{Finding, Fix};
use crate::source::{line_of, mentions};

const RULE: &str = "over-broad-visibility";

/// One `pub` item in a core crate.
struct Public {
    name: String,
    line: usize,
    /// The file the item is declared in.
    path: std::path::PathBuf,
    /// The `pub` keyword's line, so `--fix` can rewrite exactly that line.
    keyword_line: usize,
}

struct Collector {
    publics: Vec<Public>,
}

impl Collector {
    fn take(&mut self, vis: &Visibility, ident: &syn::Ident) {
        if matches!(vis, Visibility::Public(_)) {
            self.publics.push(Public {
                name: ident.to_string(),
                line: line_of(ident.span()),
                // Filled in by `publics_of`, which knows the file.
                path: std::path::PathBuf::new(),
                keyword_line: line_of(vis.span()),
            });
        }
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item(&mut self, node: &'ast Item) {
        match node {
            Item::Fn(f) => self.take(&f.vis, &f.sig.ident),
            Item::Struct(s) => self.take(&s.vis, &s.ident),
            Item::Enum(e) => self.take(&e.vis, &e.ident),
            Item::Trait(t) => self.take(&t.vis, &t.ident),
            Item::Type(t) => self.take(&t.vis, &t.ident),
            Item::Const(c) => self.take(&c.vis, &c.ident),
            Item::Static(s) => self.take(&s.vis, &s.ident),
            _ => {}
        }
        syn::visit::visit_item(self, node);
    }
}

/// Words every crate uses, so "another crate mentions it" would be noise: these
/// are never reported even when nothing names them.
///
/// `*Impl` is skipped for another reason: the concrete type is where a composition
/// root meets the contract, so it is `pub` *because* it is meant to be named from
/// outside — and a scaffold writes it before the host that names it exists.
const BOILERPLATE: [&str; 4] = ["new", "default", "from", "fmt"];

/// Every `pub` item in a core or render crate that no other crate names.
pub fn check(graph: &Graph) -> Vec<Finding> {
    let texts: Vec<(String, String)> = graph
        .crates
        .iter()
        .flat_map(|c| {
            c.source_files_all()
                .into_iter()
                .filter_map(move |p| std::fs::read_to_string(p).ok().map(|t| (c.name.clone(), t)))
        })
        .collect();

    let mut findings = Vec::new();
    for krate in graph
        .crates
        .iter()
        .filter(|c| matches!(c.class, Class::Core | Class::Render))
    {
        for public in publics_of(krate) {
            if BOILERPLATE.contains(&public.name.as_str()) || public.name.ends_with("Impl") {
                continue;
            }
            let used_elsewhere = texts
                .iter()
                .filter(|(owner, _)| owner != &krate.name)
                .any(|(_, text)| mentions(text, &public.name));
            if used_elsewhere {
                continue;
            }
            findings.push(finding(krate, &public));
        }
    }
    findings
}

fn publics_of(krate: &Crate) -> Vec<Public> {
    let mut out = Vec::new();
    for path in krate.source_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = syn::parse_str::<File>(&text) else {
            continue;
        };
        let mut collector = Collector {
            publics: Vec::new(),
        };
        syn::visit::visit_file(&mut collector, &file);
        out.extend(collector.publics.into_iter().map(|public| Public {
            path: path.clone(),
            ..public
        }));
    }
    out
}

fn finding(krate: &Crate, public: &Public) -> Finding {
    let location = format!("{}:{} ({})", krate.name, public.name, public.line);
    Finding::new(
        RULE,
        location,
        format!(
            "`pub {}` in a {} crate, and no other crate names it",
            public.name,
            krate.class.key()
        ),
        "make it `pub(crate)`: `pub` is the boundary between an implementation detail and \
         something another crate may name, and an item nothing calls does not need to be on \
         the outside of it. `gabion check --fix` applies this one.",
    )
    .with_fix(Fix::new(
        &public.path,
        public.keyword_line,
        "pub ",
        "pub(crate) ",
    ))
}

/// The set of names a fix would change, for a test or a log line.
#[must_use]
pub fn names(findings: &[Finding]) -> BTreeSet<String> {
    findings
        .iter()
        .filter(|f| f.rule == RULE)
        .map(|f| f.location.clone())
        .collect()
}
