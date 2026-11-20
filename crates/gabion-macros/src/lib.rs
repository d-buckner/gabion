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
mod server;
use cli::gen_cli;
use server::gen_server;
use syn::{parse_macro_input, ItemTrait, TraitItem, Type};

/// One method's worth of parsed info for the surface generators.
pub(crate) struct Method {
    pub(crate) ident: proc_macro2::Ident,
    pub(crate) snake: proc_macro2::Ident,
    pub(crate) input: Option<Type>, // None = unit input
    /// The `Ok` type of the declared `Result`, or the whole return type when the
    /// method does not return a `Result`.
    pub(crate) output: Type,
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
    format!("/api/{ns}.{}", m.snake)
}

/// Everything the code generators need, precomputed from the trait.
pub(crate) struct Contract {
    /// Trait name minus the `Service` suffix, snake_cased: `todo`.
    pub(crate) ns: String,
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
        trait_ident,
        trait_name,
        methods,
    };

    let server = gen_server(&c);
    let cli = gen_cli(&c);
    Ok(quote! {
        #cleaned
        #server
        #cli
    })
}

/// Parse the trait's methods and enforce the one-input-argument convention.
///
/// The retired per-method surface attributes are a hard error rather than a
/// silent no-op: an app that still writes them must learn about it at the call
/// site, not after a route it thought was hidden turns out to be public.
fn collect_methods(cleaned: &mut ItemTrait) -> syn::Result<Vec<Method>> {
    let mut methods = Vec::new();
    for item in cleaned.items.iter_mut() {
        let TraitItem::Fn(m) = item else { continue };
        reject_retired_surface_attrs(m)?;

        let mut input_ty: Option<Type> = None;
        for arg in m.sig.inputs.iter() {
            let syn::FnArg::Typed(pt) = arg else { continue };
            if input_ty.is_some() {
                return Err(syn::Error::new_spanned(
                    pt,
                    "#[service] methods take exactly one input argument (use `()` for none)",
                ));
            }
            let is_unit = matches!(&*pt.ty, Type::Tuple(t) if t.elems.is_empty());
            if !is_unit {
                input_ty = Some((*pt.ty).clone());
            }
        }

        let (summary, description) = doc_summary(&m.attrs);
        methods.push(Method {
            ident: m.sig.ident.clone(),
            snake: format_ident!("{}", to_snake(&m.sig.ident.to_string())),
            input: input_ty,
            output: ok_type(&m.sig.output),
            summary,
            description,
        });
    }
    Ok(methods)
}

/// Pull the `T` out of `Result<T, E>`; anything else is its own `T`.
fn ok_type(output: &syn::ReturnType) -> Type {
    let syn::ReturnType::Type(_, ty) = output else {
        return syn::parse_quote!(());
    };
    if let Type::Path(p) = &**ty {
        if let Some(last) = p.path.segments.last() {
            if last.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                    if let Some(syn::GenericArgument::Type(ok)) = args.args.first() {
                        return ok.clone();
                    }
                }
            }
        }
    }
    (**ty).clone()
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
