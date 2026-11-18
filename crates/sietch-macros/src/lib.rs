//! `#[service]` — one trait, four surfaces.
//!
//! Annotates a service trait. Generates (feature-gated in the host api crate):
//! - the trait itself, made `async_trait` (dyn-compatible)
//! - `server` feature: an axum `Router` with one POST per exposed method
//!   (`POST /<ns>.<method>`), JSON in, `Result` envelope out, error enum ->
//!   HTTP status via `ServiceError::kind_code`.
//! - `cli` feature: a clap `Command` + async dispatch; input arrives as
//!   `--json '<input>'` (v1), error enum -> process exit code.
//!
//! Method convention (v1): `async fn name(&self, input: T) -> Result<R, ServiceError<E>>`
//! — exactly one input argument (use `()` for none). `#[internal]` opts a method
//! out of all generated surfaces.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemTrait, TraitItem, Type};

struct Method {
    ident: proc_macro2::Ident,
    snake: proc_macro2::Ident,
    input: Option<Type>, // None = unit input
    exposed: bool,
}

#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemTrait);
    match expand(input) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
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
    let trait_ident = input.ident.clone();
    let trait_name = trait_ident.to_string();
    let ns_src = trait_name.strip_suffix("Service").unwrap_or(&trait_name);
    let ns = to_snake(ns_src);

    let mut methods: Vec<Method> = Vec::new();
    let mut cleaned = input.clone();
    cleaned.attrs.push(syn::parse_quote!(#[::sietch::async_trait]));

    for item in cleaned.items.iter_mut() {
        if let TraitItem::Fn(m) = item {
            let exposed = !input_allows_internal(&m);
            // strip sietch attrs from the emitted trait
            m.attrs.retain(|a| {
                !(a.path().is_ident("internal") || a.path().is_ident("surface"))
            });
            // find the single non-self arg
            let mut input_ty: Option<Type> = None;
            for sig_arg in m.sig.inputs.iter() {
                if let syn::FnArg::Typed(pt) = sig_arg {
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
            }
            let snake = format_ident!("{}", to_snake(&m.sig.ident.to_string()));
            methods.push(Method {
                ident: m.sig.ident.clone(),
                snake,
                input: input_ty,
                exposed,
            });
        }
    }

    let server_mod = format_ident!("__{}_server", ns);
    let cli_mod = format_ident!("__{}_cli", ns);
    let router_fn = format_ident!("{}_router", ns);
    let cli_command_fn = format_ident!("{}_cli_command", ns);
    let cli_dispatch_fn = format_ident!("{}_cli_dispatch", ns);

    // ---- server (axum) ----
    let exp: Vec<&Method> = methods.iter().filter(|m| m.exposed).collect();

    let handlers = exp.iter().map(|m| {
        let id = &m.ident;
        let hid = format_ident!("__h_{}", m.snake);
        let parse = match &m.input {
            Some(ty) => quote! {
                let input: #ty = match ::sietch::serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(err) => {
                        return (
                            ::axum::http::StatusCode::BAD_REQUEST,
                            ::axum::Json(::sietch::serde_json::json!({
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
            // route registered as #route
        }
    });
    let routes = exp.iter().map(|m| {
        let hid = format_ident!("__h_{}", m.snake);
        let route = format!("/{}/{}", ns, m.snake);
        quote! { .route(#route, ::axum::routing::post(#hid)) }
    });

    let server_ts = quote! {
        #[doc(hidden)]
        #[cfg(feature = "server")]
        pub mod #server_mod {
            #![allow(missing_docs, clippy::all)]
            use super::*;
            use ::axum::extract::State;
            use ::axum::response::IntoResponse;

            #[derive(Clone)]
            pub struct Svc(
                pub ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            );

            #(#handlers)*

            /// One POST per exposed method: `POST /<ns>.<method>`.
            pub fn #router_fn(
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            ) -> ::axum::Router {
                ::axum::Router::new()
                    #(#routes)*
                    .with_state(Svc(svc))
            }
        }
    };

    // ---- cli (clap) ----
    let subcommands = exp.iter().map(|m| {
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
    let dispatch_arms = exp.iter().map(|m| {
        let name = m.snake.to_string();
        let id = &m.ident;
        let call = match &m.input {
            Some(ty) => quote! {
                {
                    let raw = sm.get_one::<String>("json").cloned().unwrap_or_else(|| "null".into());
                    let input: #ty = ::sietch::serde_json::from_str(&raw).map_err(|e| {
                        ::sietch::cli::CliExit { code: 2, message: format!("bad --json: {e}") }
                    })?;
                    svc.#id(input).await
                }
            },
            None => quote! { svc.#id(()).await },
        };
        quote! {
            Some((#name, sm)) => #call
                .map(|v| ::sietch::serde_json::to_string_pretty(&v).unwrap_or_default())
                .map_err(|e| ::sietch::cli::CliExit {
                    code: e.kind_code.exit_code(),
                    message: e.message.clone(),
                }),
        }
    });

    let cli_ts = quote! {
        #[doc(hidden)]
        #[cfg(feature = "cli")]
        pub mod #cli_mod {
            #![allow(missing_docs, clippy::all)]
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
            ) -> ::core::result::Result<String, ::sietch::cli::CliExit> {
                match m.subcommand() {
                    #(#dispatch_arms)*
                    _ => Err(::sietch::cli::CliExit {
                        code: 2,
                        message: format!("unknown subcommand for {}", #ns),
                    }),
                }
            }
        }
    };

    Ok(quote! {
        #cleaned
        #server_ts
        #cli_ts
    })
}

fn input_allows_internal(m: &syn::TraitItemFn) -> bool {
    m.attrs.iter().any(|a| a.path().is_ident("internal"))
}
