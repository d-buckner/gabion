//! `#[service]` — one trait, four surfaces.
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
//!   HTTP status via `ServiceError::kind_code`.
//! - `cli` feature: a clap `Command` + async dispatch; input arrives as
//!   `--json '<input>'` (v1), error enum -> process exit code.
//!
//! Method convention (v1): `async fn name(&self, input: T) -> Result<R, ServiceError<E>>`
//! — exactly one input argument (use `()` for none).

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemTrait, TraitItem, Type};

/// One method's worth of parsed info for the surface generators.
struct Method {
    ident: proc_macro2::Ident,
    snake: proc_macro2::Ident,
    input: Option<Type>, // None = unit input
}

/// Everything the code generators need, precomputed from the trait.
struct Contract {
    /// Trait name minus the `Service` suffix, snake_cased: `todo`.
    ns: String,
    trait_ident: proc_macro2::Ident,
    trait_name: String,
    methods: Vec<Method>,
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

        methods.push(Method {
            ident: m.sig.ident.clone(),
            snake: format_ident!("{}", to_snake(&m.sig.ident.to_string())),
            input: input_ty,
        });
    }
    Ok(methods)
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

/// axum router: one POST per method, error enum -> HTTP status.
fn gen_server(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_ident = &c.trait_ident;
    let server_mod = format_ident!("__{}_server", ns);
    let router_fn = format_ident!("{}_router", ns);
    let exposed = &c.methods;

    let handlers = exposed.iter().map(|m| {
        let id = &m.ident;
        let hid = format_ident!("__h_{}", m.snake);
        let parse = match &m.input {
            Some(ty) => quote! {
                let input: #ty = match ::gabion::serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(err) => {
                        return (
                            ::axum::http::StatusCode::BAD_REQUEST,
                            ::axum::Json(::gabion::serde_json::json!({
                                "kind": "invalid_input",
                                "message": err.to_string(),
                            })),
                        )
                            .into_response()
                    }
                };
            },
            None => quote! { let input = (); let _ = body; },
        };
        quote! {
            async fn #hid(
                State(Svc(svc)): State<Svc>,
                body: ::axum::body::Bytes,
            ) -> ::axum::response::Response {
                #parse
                match svc.#id(input).await {
                    Ok(v) => (::axum::http::StatusCode::OK, ::axum::Json(&v)).into_response(),
                    Err(e) => {
                        let status = ::axum::http::StatusCode::from_u16(e.kind_code.http_status())
                            .unwrap_or(::axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                        (status, ::axum::Json(&e)).into_response()
                    }
                }
            }
        }
    });

    let routes = exposed.iter().map(|m| {
        let hid = format_ident!("__h_{}", m.snake);
        let route = format!("/{}/{}", ns, m.snake);
        quote! { .route(#route, ::axum::routing::post(#hid)) }
    });

    quote! {
        #[doc(hidden)]
        #[cfg(feature = "server")]
        pub mod #server_mod {
            #![allow(missing_docs, clippy::all, clippy::pedantic)]
            use super::*;
            use ::axum::extract::State;
            use ::axum::response::IntoResponse;

            #[derive(Clone)]
            pub struct Svc(
                pub ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            );

            #(#handlers)*

            /// One POST per method: `POST /<ns>.<method>`.
            pub fn #router_fn(
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            ) -> ::axum::Router {
                ::axum::Router::new()
                    #(#routes)*
                    .with_state(Svc(svc))
            }
        }
    }
}

/// clap subcommand tree + async dispatch, error enum -> exit code.
fn gen_cli(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_name = &c.trait_name;
    let trait_ident = &c.trait_ident;
    let cli_mod = format_ident!("__{}_cli", ns);
    let cli_command_fn = format_ident!("{}_cli_command", ns);
    let cli_dispatch_fn = format_ident!("{}_cli_dispatch", ns);
    let exposed = &c.methods;

    let subcommands = exposed.iter().map(|m| {
        let name = m.snake.to_string();
        match &m.input {
            Some(_) => quote! {
                ::clap::Command::new(#name).arg(
                    ::clap::Arg::new("json").long("json").required(true)
                        .help("method input as a JSON value"),
                )
            },
            None => quote! { ::clap::Command::new(#name) },
        }
    });

    let dispatch_arms = exposed.iter().map(|m| {
        let name = m.snake.to_string();
        let id = &m.ident;
        let call = match &m.input {
            Some(ty) => quote! {
                {
                    let raw = sm.get_one::<String>("json").cloned().unwrap_or_else(|| "null".into());
                    let input: #ty = ::gabion::serde_json::from_str(&raw).map_err(|e| {
                        ::gabion::cli::CliExit { code: 2, message: format!("bad --json: {e}") }
                    })?;
                    svc.#id(input).await
                }
            },
            None => quote! { svc.#id(()).await },
        };
        quote! {
            Some((#name, sm)) => #call
                .map(|v| ::gabion::serde_json::to_string_pretty(&v).unwrap_or_default())
                .map_err(|e| ::gabion::cli::CliExit {
                    code: e.kind_code.exit_code(),
                    message: e.message.clone(),
                }),
        }
    });

    quote! {
        #[doc(hidden)]
        #[cfg(feature = "cli")]
        pub mod #cli_mod {
            #![allow(missing_docs, clippy::all, clippy::pedantic)]
            use super::*;

            /// clap subcommand tree for this service.
            pub fn #cli_command_fn() -> ::clap::Command {
                ::clap::Command::new(#ns).about(concat!("", stringify!(#trait_name), " operations"))
                    .subcommands([#(#subcommands),*])
            }

            /// Dispatch a matched subcommand against the service.
            pub async fn #cli_dispatch_fn(
                svc: &(dyn #trait_ident + Send + Sync),
                m: &::clap::ArgMatches,
            ) -> ::core::result::Result<String, ::gabion::cli::CliExit> {
                match m.subcommand() {
                    #(#dispatch_arms)*
                    _ => Err(::gabion::cli::CliExit {
                        code: 2,
                        message: format!("unknown subcommand for {}", #ns),
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
