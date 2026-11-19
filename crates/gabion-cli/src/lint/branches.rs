//! The branch-count gate. clippy's own `too_many_branches` is hardcoded at 42
//! branches, is not configurable, and is not even a known lint name on every
//! toolchain — so the real gate (max-branches, default 10) counts them from the
//! AST here.

use std::path::Path;

use syn::visit::{self, Visit};
use syn::File;

use crate::graph::Crate;
use crate::report::Finding;
use crate::source::line_of;

// --- branch count ----------------------------------------------------------
//
// clippy's `too_many_branches` is hardcoded at 42 and is not a known lint name
// on every toolchain, so the real gate counts branches from the AST here.

struct BranchCounter {
    limit: usize,
    over: Vec<(usize, String, usize)>,
    current: Option<(String, usize)>,
}

impl BranchCounter {
    fn bump(&mut self) {
        if let Some((_, count)) = &mut self.current {
            *count += 1;
        }
    }

    fn enter_fn(&mut self, name: &str, line: usize, body: &syn::Block, limit: usize) {
        let outer = self.current.replace((name.to_string(), 0));
        syn::visit::visit_block(self, body);
        if let Some((fn_name, count)) = self.current.take() {
            if count > limit {
                self.over.push((line, fn_name, count));
            }
        }
        self.current = outer;
    }
}

impl<'ast> Visit<'ast> for BranchCounter {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.enter_fn(
            &node.sig.ident.to_string(),
            line_of(node.sig.ident.span()),
            &node.block,
            self.limit,
        );
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.enter_fn(
            &node.sig.ident.to_string(),
            line_of(node.sig.ident.span()),
            &node.block,
            self.limit,
        );
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.bump();
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        // Count arms, not the match itself: a 42-arm match is 42 branches, and
        // counting the expression as one is how the gate gets walked around.
        for _ in &node.arms {
            self.bump();
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.bump();
        syn::visit::visit_expr_loop(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.bump();
        syn::visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.bump();
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        self.bump();
        syn::visit::visit_expr_try(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) {
            self.bump();
        }
        syn::visit::visit_expr_binary(self, node);
    }
}

pub(crate) fn branch_rule(krate: &Crate, path: &Path, file: &File, limit: usize) -> Vec<Finding> {
    let mut v = BranchCounter {
        limit,
        over: Vec::new(),
        current: None,
    };
    visit::visit_file(&mut v, file);
    v.over
        .into_iter()
        .map(|(line, name, count)| {
            Finding::new(
                "too-many-branches",
                krate.at(path, line),
                format!("`{name}` has {count} branches, limit is {limit}"),
                "branch-heavy code is where an agent hides a shortcut: extract the dispatch into a match on a named enum, or split the fn.",
            )
        })
        .collect()
}
