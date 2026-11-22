//! The names a surface spells: the wire name of an operation, the route it is
//! served on, and the namespace derived from a trait name.
//!
//! Split from `lib.rs` because these three are one job — *what this operation is
//! called on the wire* — and the attribute plumbing next door is another. Every
//! generator reads them from here, so a surface cannot invent its own spelling.
//!
//! The namespace is the load-bearing one: it is the route prefix, the MCP tool
//! prefix, the `ns` on every frame, the key of the stream wall on both ends and the
//! `x-gabion-streams` key in the OpenAPI document.

use crate::Method;

/// The route a method is served on: `POST /api/todo.create`.
///
/// The spec's wire decision (spec §5) is one POST per method, dotted under
/// `/api`, and this is the only place it is computed — so the router and the
/// OpenAPI document cannot disagree about it.
pub(crate) fn route(ns: &str, m: &Method) -> String {
    format!("/api/{}", tool_name(ns, m))
}

/// The operation's name without a transport in it: `todo.create`. The route is
/// this under `/api`, and the MCP tool is this exactly, so a client reading the
/// OpenAPI document and an agent reading `tools/list` name the same operation.
pub(crate) fn tool_name(ns: &str, m: &Method) -> String {
    format!("{ns}.{}", m.snake)
}

/// `TodoId` -> `todo_id`, `HTTPApi` -> `http_api`.
///
/// A run of capitals is one word, so the break goes before its *last* capital —
/// the one that starts the next word — not before every capital. The namespace
/// this produces is not cosmetic: it is the route prefix, the MCP tool prefix,
/// the `ns` on every frame and the stream-wall key, so `HTTPService` becoming
/// `h_t_t_p` would be a wire surface nobody can type.
///
/// Digits are not letters: they neither open a word nor close one, which keeps
/// `H2O` at `h2_o` and `Todo2` at `todo2`.
pub(crate) fn to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let starts_word_after_capitals = prev.is_some_and(char::is_uppercase)
            && chars.peek().is_some_and(|next| next.is_lowercase());
        let starts_word = c.is_uppercase()
            && prev.is_some()
            && (!prev.is_some_and(char::is_uppercase) || starts_word_after_capitals);
        if starts_word {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
        prev = Some(c);
    }
    out
}
