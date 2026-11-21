//! The CLI surface: a clap subcommand tree and the dispatch that runs one op
//! against a service implementation.

use quote::{format_ident, quote};

use crate::Contract;

/// Global identity flags on the service command. They exist so a local caller
/// can say who it is without a hand-written adapter: the surface builds the same
/// `Ctx` every other surface builds.
fn ctx_args() -> proc_macro2::TokenStream {
    quote! {
        .arg(
            ::clap::Arg::new("principal").long("principal").global(true)
                .help("caller identity carried into every op on this service"),
        )
        .arg(
            ::clap::Arg::new("tenant").long("tenant").global(true)
                .help("tenant the call is scoped to"),
        )
        .arg(
            ::clap::Arg::new("trace").long("trace").global(true)
                .help("trace id carried into every op on this service"),
        )
    }
}

/// clap subcommand tree + async dispatch, error enum -> exit code.
pub(crate) fn gen_cli(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_name = &c.trait_name;
    let trait_ident = &c.trait_ident;
    let cli_mod = format_ident!("__{}_cli", ns);
    let cli_command_fn = format_ident!("{}_cli_command", ns);
    let cli_dispatch_fn = format_ident!("{}_cli_dispatch", ns);
    let exposed = &c.methods;
    let ctx_args = ctx_args();

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

    let dispatch_arms = dispatch_arms(c);

    quote! {
        #[doc(hidden)]
        #[cfg(feature = "cli")]
        pub mod #cli_mod {
            #![allow(missing_docs, clippy::all, clippy::pedantic)]
            use super::*;

            /// clap subcommand tree for this service.
            pub fn #cli_command_fn() -> ::clap::Command {
                ::clap::Command::new(#ns).about(concat!("", stringify!(#trait_name), " operations"))
                    #ctx_args
                    .subcommands([#(#subcommands),*])
            }

            /// Dispatch a matched subcommand against the service.
            ///
            /// `base` is the context the host built (usually anonymous); the
            /// service's own `--principal` / `--tenant` / `--trace` flags narrow
            /// it, so a caller that names itself is not silently anonymous.
            pub async fn #cli_dispatch_fn(
                svc: &(dyn #trait_ident + Send + Sync),
                base: &::gabion::ctx::Ctx,
                m: &::clap::ArgMatches,
            ) -> ::core::result::Result<String, ::gabion::cli::CliExit> {
                let mut ctx = base.clone();
                if let Some(v) = m.get_one::<String>("principal") {
                    ctx = ctx.with_principal(v.clone());
                }
                if let Some(v) = m.get_one::<String>("tenant") {
                    ctx = ctx.with_tenant(v.clone());
                }
                if let Some(v) = m.get_one::<String>("trace") {
                    ctx = ctx.with_trace(v.clone());
                }
                match m.subcommand() {
                    #(#dispatch_arms)*
                    _ => Err(::gabion::cli::CliExit::new(
                        ::gabion::error::ErrorKind::InvalidInput,
                        format!("unknown subcommand for {}", #ns),
                    )),
                }
            }
        }
    }
}

/// One `match` arm per operation: read `--json`, call, render.
///
/// A listen op prints newline-delimited JSON, one item per line, and its exit code
/// still comes from the same `CliExit` projection, so a stream that failed and a
/// value that failed are told apart the same way.
fn dispatch_arms(c: &Contract) -> Vec<proc_macro2::TokenStream> {
    c.methods.iter().map(|m| {
        let name = m.snake.to_string();
        let id = &m.ident;
        let call = match &m.input {
            Some(ty) => quote! {
                {
                    let raw = sm.get_one::<String>("json").cloned().unwrap_or_else(|| "null".into());
                    let input: #ty = ::gabion::serde_json::from_str(&raw).map_err(|e| {
                        ::gabion::cli::CliExit::new(
                            ::gabion::error::ErrorKind::InvalidInput,
                            format!("bad --json: {e}"),
                        )
                    })?;
                    svc.#id(&ctx, input).await
                }
            },
            None => quote! { svc.#id(&ctx, ()).await },
        };
        let render = match m.stream {
            Some(_) => quote! {
                match #call {
                    ::core::result::Result::Ok(__stream) => {
                        ::gabion::cli::render_ndjson(__stream).await
                    }
                    ::core::result::Result::Err(e) => {
                        ::core::result::Result::Err(::gabion::cli::CliExit::from_service(&e))
                    }
                }
            },
            None => quote! {
                #call
                    // One projection for every failure: the exit carries the same
                    // wire envelope the route sends, so the four surfaces compare.
                    .map_err(|e| ::gabion::cli::CliExit::from_service(&e))
                    .and_then(|v| ::gabion::cli::render_json(&v))
            },
        };
        quote! {
            Some((#name, sm)) => #render,
        }
    }).collect()
}
