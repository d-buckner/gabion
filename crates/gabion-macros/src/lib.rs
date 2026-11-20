//! `#[service]` — one trait, every surface.
//!
//! Two of the four named surfaces are built today (`cli`, `server`/`openapi`);
//! `mcp` and the typed client are planned (docs/GAPS.md §4).
//!
//! Annotates a service trait. **The trait is the boundary:** every method in it
//! is generated into every surface, with no per-method opt-in or opt-out. An op
//! that must not be remote does not belong in this trait — put it in a plain,
//! non-`#[service]` trait in the same api crate (reachable in-proc from other
//! features) or in an inherent `impl` block on the type (reachable nowhere
//! outside the feature). Adapters only ever hold `Arc<dyn TheService>`, so
//! anything outside the trait is unreachable from every surface by
//! construction, not by lint.
//!
//! Generates (feature-gated in the host api crate):
//! - the trait itself, made `async_trait` (dyn-compatible)
//! - `server` feature: an axum `Router` with one POST per method
//!   (`POST /<ns>.<method>`), JSON in, `Result` envelope out, error enum ->
//!   HTTP status via `ErrorKind::http_status`, body is a flat `WireError`.
//! - `cli` feature: a clap `Command` + async dispatch; input arrives as
//!   `--json '<input>'` (v1), error enum -> process exit code.
//!
//! Method convention (v1): `async fn name(&self, input: T) -> Result<R, ServiceError<E>>`
//! — exactly one input argument (use `()` for none).

use proc_macro::TokenStream;
use quote::{format_ident, quote};

mod cli;
mod client;
mod mcp;
mod registry;
mod server;
use cli::gen_cli;
use client::gen_client;
use mcp::gen_mcp;
use registry::gen_registry;
use server::gen_server;
use syn::{parse_macro_input, ItemTrait, TraitItem, Type};

/// One method's worth of parsed info for the surface generators.
pub(crate) struct Method {
    pub(crate) ident: proc_macro2::Ident,
    pub(crate) snake: proc_macro2::Ident,
    pub(crate) input: Option<Type>, // None = unit input
    /// The `Ok` type of the declared `Result`.
    pub(crate) output: Type,
    /// The `Err` type of the declared `Result`. The generated client needs it: a
    /// failure that crossed the wire has to come back as the app's own error
    /// type, not as `serde_json::Value`.
    pub(crate) error: Type,
    /// First line of the method's doc comment; empty when it has none.
    pub(crate) summary: String,
    /// The rest of the doc comment, empty when there is nothing beyond the first
    /// line.
    pub(crate) description: String,
}

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

/// Everything the code generators need, precomputed from the trait.
pub(crate) struct Contract {
    /// Trait name minus the `Service` suffix, snake_cased: `todo`.
    pub(crate) ns: String,
    /// Trait name minus the `Service` suffix, spelled as written: `Todo`. The
    /// generated client is `TodoClient`.
    pub(crate) base: String,
    pub(crate) trait_ident: proc_macro2::Ident,
    pub(crate) trait_name: String,
    pub(crate) methods: Vec<Method>,
}

#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    if let Err(e) = reject_service_args(&proc_macro2::TokenStream::from(attr)) {
        return e.to_compile_error().into();
    }
    let input = parse_macro_input!(item as ItemTrait);
    match expand(input) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[service]` takes no arguments. Silently ignoring `#[service(http)]` would
/// mean an author who believes they restricted a surface did not restrict it.
fn reject_service_args(attr: &proc_macro2::TokenStream) -> syn::Result<()> {
    if attr.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        attr,
        "#[service] takes no arguments: every method in the trait is generated into \
         every surface. An op that must not be remote belongs in a plain \
         non-#[service] trait, not in this one.",
    ))
}

fn to_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn expand(input: ItemTrait) -> syn::Result<proc_macro2::TokenStream> {
    let mut cleaned = input.clone();
    cleaned
        .attrs
        .push(syn::parse_quote!(#[::gabion::async_trait]));
    let trait_ident = cleaned.ident.clone();
    let trait_name = trait_ident.to_string();
    let ns_src = trait_name.strip_suffix("Service").unwrap_or(&trait_name);

    let methods = collect_methods(&mut cleaned)?;
    let c = Contract {
        ns: to_snake(ns_src),
        base: ns_src.to_string(),
        trait_ident,
        trait_name,
        methods,
    };

    let server = gen_server(&c);
    let cli = gen_cli(&c);
    let client = gen_client(&c);
    let mcp = gen_mcp(&c);
    let registration = gen_registry(&c);
    Ok(quote! {
        #cleaned
        #server
        #cli
        #client
        #mcp
        #registration
    })
}

/// Parse the trait's methods and enforce the calling convention: `&Ctx` first,
/// then at most one input argument.
///
/// The context is required rather than optional on purpose. A method without one
/// is a method whose surface cannot say who called it, and the surfaces are
/// generated, not written — so the only place to catch that is here.
fn collect_methods(cleaned: &mut ItemTrait) -> syn::Result<Vec<Method>> {
    let mut methods = Vec::new();
    for item in cleaned.items.iter_mut() {
        let TraitItem::Fn(m) = item else { continue };
        reject_retired_surface_attrs(m)?;

        let mut input_ty: Option<Type> = None;
        let mut seen_ctx = false;
        for arg in m.sig.inputs.iter() {
            let syn::FnArg::Typed(pt) = arg else { continue };
            if !seen_ctx {
                if !is_ctx(&pt.ty) {
                    return Err(syn::Error::new_spanned(
                        &pt.ty,
                        "every #[service] method takes `ctx: &Ctx` first:\n\n    \
                         async fn create(&self, ctx: &Ctx, input: T) -> Result<R, E>\n\n\
                         A call has a caller. The generated surfaces fill this in (HTTP reads its \
                         headers, MCP its `_meta`, the CLI its flags), and a method that does not \
                         take one can never be told who is calling.",
                    ));
                }
                seen_ctx = true;
                continue;
            }
            if input_ty.is_some() {
                return Err(syn::Error::new_spanned(
                    pt,
                    "#[service] methods take `&Ctx` and exactly one input argument (use `()` for none)",
                ));
            }
            let is_unit = matches!(&*pt.ty, Type::Tuple(t) if t.elems.is_empty());
            if !is_unit {
                input_ty = Some((*pt.ty).clone());
            }
        }
        if !seen_ctx {
            return Err(syn::Error::new_spanned(
                &m.sig.ident,
                "every #[service] method takes `ctx: &Ctx` first: `async fn name(&self, ctx: &Ctx, input: T)`",
            ));
        }

        let Some((output, Some(error))) = result_args(&m.sig.output) else {
            return Err(syn::Error::new_spanned(
                &m.sig.output,
                "every #[service] method returns `Result<T, E>`:\n\n    \
                 async fn create(&self, ctx: &Ctx, input: T) -> Result<R, E>\n\n\
                 An adapter that cannot express failure either invents a success or panics, \
                 so a method that cannot fail uses `Result<T, Infallible>`.",
            ));
        };
        let (summary, description) = doc_summary(&m.attrs);
        methods.push(Method {
            ident: m.sig.ident.clone(),
            snake: format_ident!("{}", to_snake(&m.sig.ident.to_string())),
            input: input_ty,
            output,
            error,
            summary,
            description,
        });
    }
    Ok(methods)
}

/// `&Ctx`, `&gabion::Ctx`, `&crate::Ctx`: the type's last segment is what the
/// convention is about, since a contract may import or fully qualify it.
fn is_ctx(ty: &Type) -> bool {
    let Type::Reference(r) = ty else {
        return false;
    };
    let Type::Path(p) = &*r.elem else {
        return false;
    };
    p.path.segments.last().is_some_and(|seg| seg.ident == "Ctx")
}

/// `Result<T, E>` -> `(T, Some(E))`; `Result<T>` -> `(T, None)`. Every other
/// return type is not a `Result` at all.
fn result_args(output: &syn::ReturnType) -> Option<(Type, Option<Type>)> {
    let syn::ReturnType::Type(_, ty) = output else {
        return None;
    };
    let Type::Path(p) = &**ty else {
        return None;
    };
    let last = p.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });
    let ok = types.next()?;
    Some((ok, types.next()))
}

/// The method's doc comment, split into a one-line summary and the rest. The
/// OpenAPI document is generated from the same trait, so the prose a reviewer
/// reads in the source is the prose a client reads in the contract.
fn doc_summary(attrs: &[syn::Attribute]) -> (String, String) {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                lines.push(s.value().trim().to_string());
            }
        }
    }
    let lines: Vec<String> = lines.into_iter().filter(|l| !l.is_empty()).collect();
    let Some((first, rest)) = lines.split_first() else {
        return (String::new(), String::new());
    };
    if rest.is_empty() {
        return (first.clone(), String::new());
    }
    (first.clone(), rest.join("\n"))
}

fn reject_retired_surface_attrs(m: &syn::TraitItemFn) -> syn::Result<()> {
    for attr in &m.attrs {
        let path = attr.path();
        if path.is_ident("surface") || path.is_ident("internal") {
            return Err(syn::Error::new_spanned(
                attr,
                "per-method surface attributes are gone: every method in a #[service] \
                 trait is exposed on every surface. Move this method to a plain \
                 non-#[service] trait if it must not be remote.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
