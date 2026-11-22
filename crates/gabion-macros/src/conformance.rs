//! The four-adapter agreement suite (spec §8): one impl, one input, four
//! surfaces, compared.
//!
//! Generation is not verification. Four adapters that are each hand-exercised can
//! each be green while disagreeing: the CLI lowercases an id, the route drops a
//! field, the client maps a conflict to an internal error. This generates the
//! comparison from the trait, so an op added there is a case every adapter has to
//! agree on — and an op the suite never drives is a rejection, not a pass.
//!
//! The four are:
//!
//! - **in-proc**: the impl, called directly;
//! - **cli**: the generated clap tree, run through the generated dispatch;
//! - **mcp**: the generated tool, over the JSON-RPC call path;
//! - **http**: the generated client, over whatever `Http` capability it was given.
//!
//! Inputs must be `Clone`: one input is handed to four adapters, and comparing
//! them on four different inputs would prove nothing.
//!
//! A listen op compares the same four surfaces, but on what came out rather than
//! on one value: the item list plus how the stream ended. §8 says a stream case is
//! a case like any other, so it goes through the same `Agreement`, and the only
//! thing that differs is how each surface is turned into a comparable value.

use quote::{format_ident, quote};

use crate::{tool_name, Contract, Method, Policy};

/// `<ns>_conformance(svc, http, base_url, cases) -> Report`.
///
/// A feature with listen ops gains a second capability argument, because a client
/// that can stream needs a `Streams` to dial with. A feature without them keeps the
/// four-argument shape it always had.
pub(crate) fn gen_conformance(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_ident = &c.trait_ident;
    let base = &c.base;
    let client_ident = format_ident!("{}Client", base);
    let mcp_mod = format_ident!("__{}_mcp", ns);
    let mcp_fn = format_ident!("{}_mcp", ns);
    let cli_mod = format_ident!("__{}_cli", ns);
    let cli_fn = format_ident!("{}_cli_command", ns);
    let dispatch_fn = format_ident!("{}_cli_dispatch", ns);
    let suite = format_ident!("{}_conformance", ns);
    let arms = c.methods.iter().map(|m| match m.stream {
        Some(policy) => stream_arm(ns, m, policy, &cli_mod, &cli_fn, &dispatch_fn),
        None => arm(ns, m, &cli_mod, &cli_fn, &dispatch_fn),
    });
    let (streams_generic, make_client) = if c.streams() {
        (
            quote! { , S: ::gabion::caps::Streams },
            quote! {
                let client = #client_ident::with_streams(
                    base_url.to_string(),
                    http,
                    streams,
                );
            },
        )
    } else {
        (
            quote! {},
            quote! { let client = #client_ident::new(base_url.to_string(), http); },
        )
    };
    let streams_arg = c.streams().then(|| quote! { streams: S, });

    quote! {
        /// Drive every case through all four surfaces and compare the answers.
        ///
        /// The suite is generated, the cases are the app's: only the app knows
        /// what a call means. A disagreement names the surface that differed and
        /// what it said, because "the adapters disagree" is not actionable.
        #[cfg(feature = "conformance")]
        #[allow(clippy::too_many_lines)] // one match arm per op, by construction
        pub async fn #suite<H: ::gabion::caps::Http #streams_generic>(
            svc: ::std::sync::Arc<dyn #trait_ident + Send + Sync>,
            http: H,
            #streams_arg
            base_url: &str,
            cases: &[::gabion::conformance::Case],
        ) -> ::gabion::conformance::Report {
            #make_client
            let mcp = #mcp_mod::#mcp_fn(svc.clone());
            let mcp_server = ::gabion::mcp::Server::new()
                .with(::std::sync::Arc::new(#mcp_mod::#mcp_fn(svc.clone())));
            let mut report = ::gabion::conformance::Report::default();
            for case in cases {
                // One identity for all four, so a surface that drops it is a
                // disagreement rather than a difference between cases.
                let ctx = ::gabion::ctx::Ctx::anonymous().with_principal("conformance");
                let (in_proc, cli, mcp_out, over_http) = match case.op {
                    #(#arms)*
                    other => {
                        report.reject(other, "no adapter implements this op");
                        continue;
                    }
                };
                report.record(::gabion::conformance::Agreement::new(
                    case.op, in_proc, cli, mcp_out, over_http,
                ));
            }
            report
        }
    }
}

/// One op's four calls, written out per surface rather than factored into a
/// closure: the four are *different code paths*, and hiding them behind one shape
/// is how a suite stops testing what it claims.
fn arm(
    ns: &str,
    m: &Method,
    cli_mod: &proc_macro2::Ident,
    cli_fn: &proc_macro2::Ident,
    dispatch_fn: &proc_macro2::Ident,
) -> proc_macro2::TokenStream {
    let op = tool_name(ns, m);
    let id = &m.ident;
    let subcommand = m.snake.to_string();
    let (parse, cli_json, inproc, over_http) = match &m.input {
        Some(ty) => (
            quote! {
                let input: #ty = match ::gabion::serde_json::from_value(case.input.clone()) {
                    ::core::result::Result::Ok(v) => v,
                    ::core::result::Result::Err(e) => {
                        report.reject(#op, format!("input does not parse: {e}"));
                        continue;
                    }
                };
            },
            quote! { &["app", #ns, #subcommand, "--json", &case.input.to_string()] },
            quote! { svc.#id(&ctx, input.clone()).await },
            quote! { client.#id(&ctx, input.clone()).await },
        ),
        None => (
            quote! {},
            quote! { &["app", #ns, #subcommand] },
            quote! { svc.#id(&ctx, ()).await },
            quote! { client.#id(&ctx, ()).await },
        ),
    };
    let cli = plain_cli(&cli_json, cli_mod, cli_fn, dispatch_fn);
    quote! {
        #op => {
            #parse
            let in_proc = match #inproc {
                ::core::result::Result::Ok(v) => ::gabion::conformance::ok_value(&v),
                ::core::result::Result::Err(e) => ::gabion::conformance::err_value(&e),
            };
            let cli = #cli;
            let mcp_out = match ::gabion::mcp::Service::call(
                &mcp, &ctx, #op, case.input.clone(),
            )
            .await
            {
                ::core::result::Result::Ok(v) => v,
                ::core::result::Result::Err(wire) => ::gabion::conformance::wire_value(&wire),
            };
            let over_http = match #over_http {
                ::core::result::Result::Ok(v) => ::gabion::conformance::ok_value(&v),
                ::core::result::Result::Err(e) => ::gabion::conformance::err_value(&e),
            };
            (in_proc, cli, mcp_out, over_http)
        }
    }
}

/// The CLI half of an ordinary case: one value, printed as JSON.
fn plain_cli(
    cli_json: &proc_macro2::TokenStream,
    cli_mod: &proc_macro2::Ident,
    cli_fn: &proc_macro2::Ident,
    dispatch_fn: &proc_macro2::Ident,
) -> proc_macro2::TokenStream {
    quote! {
        {
            let args: &[&str] = #cli_json;
            match ::clap::Command::new("app")
                .subcommand(#cli_mod::#cli_fn())
                .try_get_matches_from(args)
            {
                ::core::result::Result::Ok(m) => match m.subcommand() {
                    ::core::option::Option::Some((_, service)) => {
                        let mut text = String::new();
                        match #cli_mod::#dispatch_fn(svc.as_ref(), &ctx, service, &mut text)
                            .await
                        {
                            ::core::result::Result::Ok(()) => {
                                ::gabion::conformance::text_value(&text)
                            }
                            ::core::result::Result::Err(exit) => {
                                ::gabion::conformance::wire_value(&exit.wire)
                            }
                        }
                    }
                    ::core::option::Option::None => {
                        ::gabion::conformance::text_value("<no subcommand matched>")
                    }
                },
                ::core::result::Result::Err(e) => {
                    ::gabion::conformance::text_value(&e.to_string())
                }
            }
        }
    }
}

/// One listen op's four calls. Each surface delivers items in its own shape — a
/// stream in-proc, NDJSON on stdout, progress notifications over MCP, a decoded
/// stream over the socket — so each is collected into the same
/// `{items, end}` value before the comparison.
fn stream_arm(
    ns: &str,
    m: &Method,
    policy: Policy,
    cli_mod: &proc_macro2::Ident,
    cli_fn: &proc_macro2::Ident,
    dispatch_fn: &proc_macro2::Ident,
) -> proc_macro2::TokenStream {
    let op = tool_name(ns, m);
    let id = &m.ident;
    let subcommand = m.snake.to_string();
    let _ = policy;
    let (parse, cli_json) = match &m.input {
        Some(ty) => (
            quote! {
                let input: #ty = match ::gabion::serde_json::from_value(case.input.clone()) {
                    ::core::result::Result::Ok(v) => v,
                    ::core::result::Result::Err(e) => {
                        report.reject(#op, format!("input does not parse: {e}"));
                        continue;
                    }
                };
            },
            quote! { &["app", #ns, #subcommand, "--json", &case.input.to_string()] },
        ),
        None => (quote! {}, quote! { &["app", #ns, #subcommand] }),
    };
    let cli = stream_cli(&cli_json, cli_mod, cli_fn, dispatch_fn);
    let mcp = stream_mcp(&op);
    quote! {
        #op => {
            #parse
            let in_proc = match svc.#id(&ctx, input.clone()).await {
                ::core::result::Result::Ok(stream) => ::gabion::conformance::stream_value(
                    ::gabion::conformance::collect(stream).await,
                    ::gabion::serde_json::json!("done"),
                ),
                ::core::result::Result::Err(e) => {
                    ::gabion::conformance::stream_refused(::gabion::conformance::err_value(&e))
                }
            };
            let cli = #cli;
            let mcp_out = #mcp;
            let over_http = match client.#id(&ctx, input.clone()).await {
                ::core::result::Result::Ok(stream) => ::gabion::conformance::stream_value(
                    ::gabion::conformance::collect(stream).await,
                    ::gabion::serde_json::json!("done"),
                ),
                ::core::result::Result::Err(e) => {
                    ::gabion::conformance::stream_refused(::gabion::conformance::err_value(&e))
                }
            };
            (in_proc, cli, mcp_out, over_http)
        }
    }
}

/// The CLI half of a stream case: run the subcommand, read the NDJSON it printed.
fn stream_cli(
    cli_json: &proc_macro2::TokenStream,
    cli_mod: &proc_macro2::Ident,
    cli_fn: &proc_macro2::Ident,
    dispatch_fn: &proc_macro2::Ident,
) -> proc_macro2::TokenStream {
    quote! {
        {
            let args: &[&str] = #cli_json;
            match ::clap::Command::new("app")
                .subcommand(#cli_mod::#cli_fn())
                .try_get_matches_from(args)
            {
                ::core::result::Result::Ok(m) => match m.subcommand() {
                    ::core::option::Option::Some((_, service)) => {
                        let mut text = String::new();
                        match #cli_mod::#dispatch_fn(svc.as_ref(), &ctx, service, &mut text)
                            .await
                        {
                            ::core::result::Result::Ok(()) => {
                                ::gabion::conformance::ndjson_value(&text)
                            }
                            ::core::result::Result::Err(exit) => {
                                ::gabion::conformance::stream_refused(
                                    ::gabion::conformance::wire_value(&exit.wire),
                                )
                            }
                        }
                    }
                    ::core::option::Option::None => {
                        ::gabion::conformance::refused_value("<no subcommand matched>")
                    }
                },
                ::core::result::Result::Err(e) => {
                    ::gabion::conformance::refused_value(&e.to_string())
                }
            }
        }
    }
}

/// The MCP half of a stream case: a `tools/call` with a progress token, then the
/// notifications and the answer read back together.
fn stream_mcp(op: &str) -> proc_macro2::TokenStream {
    quote! {
        {
            let mut sink = ::gabion::mcp::Collect::default();
            let request = ::gabion::serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": #op,
                    "arguments": case.input,
                    "_meta": { "progressToken": "conformance" },
                },
            });
            match mcp_server.handle_with(&request.to_string(), &mut sink).await {
                ::core::option::Option::Some(reply) => {
                    ::gabion::conformance::mcp_stream_value(&reply, &sink.lines)
                }
                ::core::option::Option::None => ::gabion::conformance::refused_value("<no reply>"),
            }
        }
    }
}
