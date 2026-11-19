//! The CLI surface: a clap subcommand tree and the dispatch that runs one op
//! against a service implementation.

use quote::{format_ident, quote};

use crate::Contract;

/// clap subcommand tree + async dispatch, error enum -> exit code.
pub(crate) fn gen_cli(c: &Contract) -> proc_macro2::TokenStream {
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
                .map_err(|e| ::gabion::cli::CliExit {
                    code: e.kind.exit_code(),
                    message: e.message.clone(),
                })
                .and_then(|v| ::gabion::cli::render_json(&v)),
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
