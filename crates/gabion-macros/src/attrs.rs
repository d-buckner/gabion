//! The one place a gabion service trait's `async_trait` is spelled.
//!
//! Split out because the rule it encodes is a target rule, not a code-generation
//! rule: nothing in the generators reads it, and every generator must agree with it.

/// The `#[async_trait]` a `#[service]` trait and every impl of one must carry.
///
/// A stream policy marker sits in a method's `Output`, so on wasm the future
/// `async_trait` boxes for that method stops being `Send` there — and `async_trait`
/// compares an impl against its trait for equality, not for subtyping, so once one
/// method streams every method of that trait loses `Send` on that target
/// (`docs/streaming.md` §4.2). Both `cfg`s are here so no core writes them.
pub(crate) fn async_trait_attrs() -> Vec<syn::Attribute> {
    vec![
        syn::parse_quote!(#[cfg_attr(not(target_arch = "wasm32"), ::gabion::async_trait)]),
        syn::parse_quote!(#[cfg_attr(target_arch = "wasm32", ::gabion::async_trait(?Send))]),
    ]
}
