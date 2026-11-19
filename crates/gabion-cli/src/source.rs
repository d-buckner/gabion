//! Source infrastructure shared by the rules: exact comment tokenization,
//! span-to-line mapping, and the regions that grant suppressions.
//!
//! Comments come from `rustc_lexer` — the actual rustc tokenizer — so `///` vs
//! `//` vs a string containing `//` is told apart exactly rather than guessed at
//! by a hand-rolled scanner. Structure comes from `syn`, with `span-locations`
//! giving line numbers on stable.

use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, File, Meta, Token};

/// A comment with its line and whether it is a doc comment.
pub struct Comment {
    pub line: usize,
    pub doc: bool,
    pub text: String,
}

/// Line ranges that grant a suppression (test modules, generated modules).
#[derive(Default)]
pub struct Regions {
    pub test: Vec<(usize, usize)>,
    pub generated: Vec<(usize, usize)>,
}

impl Regions {
    #[must_use]
    pub fn covers(&self, list: &[(usize, usize)], line: usize) -> bool {
        list.iter().any(|(a, b)| line >= *a && line <= *b)
    }
}

/// Every comment in the file, with its line and doc-ness.
#[must_use]
pub fn scan_comments(content: &str) -> Vec<Comment> {
    let mut out = Vec::new();
    let mut off = 0usize;
    let mut line = 1usize;
    // rustc_lexer 0.1 reports `len` in bytes (its own iterator slices by it),
    // so byte-offset slicing is exact even with non-ASCII in comments.
    for tok in rustc_lexer::tokenize(content) {
        let text = &content[off..off + tok.len];
        let is_comment = matches!(
            tok.kind,
            rustc_lexer::TokenKind::LineComment | rustc_lexer::TokenKind::BlockComment { .. }
        );
        if is_comment {
            // 0.1 has no doc_style field; a doc comment starts with `///`,
            // `//!`, or `/**`.
            let doc = text.starts_with("///") || text.starts_with("//!") || text.starts_with("/**");
            out.push(Comment {
                line,
                doc,
                text: text.to_string(),
            });
        }
        line += text.matches('\n').count();
        off += tok.len;
    }
    out
}

pub(crate) fn line_of(span: proc_macro2::Span) -> usize {
    span.start().line
}

pub(crate) fn span_range(span: proc_macro2::Span) -> (usize, usize) {
    (span.start().line, span.end().line)
}

/// `#[cfg(test)]` on a module — tests may unwrap and carry notes.
pub(crate) fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .any(|a| nested_paths(a).contains(&"test".to_string()))
}

/// Nested idents of `#[attr(a, b)]`, rendered as `a::b` paths.
pub(crate) fn nested_paths(attr: &Attribute) -> Vec<String> {
    let Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    let Ok(metas) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
        return Vec::new();
    };
    metas
        .iter()
        .filter_map(|m| match m {
            Meta::Path(p) => Some(
                p.segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
            ),
            _ => None,
        })
        .collect()
}

struct RegionCollector {
    regions: Regions,
}

impl<'ast> Visit<'ast> for RegionCollector {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if let Some((brace, _)) = &node.content {
            let range = span_range(brace.span.join());
            if has_cfg_test(&node.attrs) {
                // The grant has to cover the attributes *on* the mod too: the
                // idiomatic `#[cfg(test)] #[allow(clippy::unwrap_used)] mod tests`
                // puts the allow outside the brace range.
                let start = node.attrs.first().map_or_else(
                    || line_of(node.ident.span()),
                    |a| line_of(a.pound_token.span),
                );
                self.regions.test.push((start, range.1));
            }
            if node.ident.to_string().starts_with("__") {
                self.regions.generated.push(range);
            }
        }
        syn::visit::visit_item_mod(self, node);
    }
}

pub(crate) fn regions_of(file: &File) -> Regions {
    let mut v = RegionCollector {
        regions: Regions::default(),
    };
    visit::visit_file(&mut v, file);
    v.regions
}
