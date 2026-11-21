//! The stream surface: one adapter per feature that a host pumps over the shared
//! `/api/ws` connection.
//!
//! The adapter dispatches by op name, the same way the MCP adapter does, and it is
//! generated from the same `Method` list — so a listen op cannot be served on the
//! WebSocket while being missing from `tools/list`, and the reverse. The policy it
//! reports comes from the return marker, not from anything the host configures.

use quote::{format_ident, quote};

use crate::{tool_name, Contract, Method};

pub(crate) fn gen_ws(c: &Contract) -> proc_macro2::TokenStream {
    if !c.streams() {
        // A feature with no listen op gets no adapter, and so no route. That is the
        // "byte-identical to today" half of docs/streaming.md §5, made structural.
        return proc_macro2::TokenStream::new();
    }
    let ns = &c.ns;
    let base = &c.base;
    let trait_ident = &c.trait_ident;
    let ns_literal = ns.as_str();
    let ws_mod = format_ident!("__{}_ws", ns);
    let ws_ident = format_ident!("{}Ws", base);
    let ws_fn = format_ident!("{}_ws", ns);
    let ops = c.methods.iter().map(|m| op(ns, m));
    let call_arms = c
        .methods
        .iter()
        .filter(|m| m.stream.is_none())
        .map(|m| call_arm(ns, m));
    let open_arms = c.methods.iter().filter_map(|m| open_arm(ns, m));

    quote! {
        #[doc(hidden)]
        #[cfg(feature = "server")]
        pub mod #ws_mod {
            #![allow(missing_docs, clippy::all, clippy::pedantic)]
            use super::*;

            /// One service as the stream surface sees it. The host pumps it; this
            /// never touches a socket.
            pub struct #ws_ident {
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            }

            /// The stream adapter for one impl.
            #[must_use]
            pub fn #ws_fn(
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            ) -> #ws_ident {
                #ws_ident { svc }
            }

            impl ::gabion::ws::WsService for #ws_ident {
                fn ns(&self) -> &str {
                    #ns_literal
                }

                fn ops(&self) -> ::std::vec::Vec<::gabion::ws::Op> {
                    ::std::vec![#(#ops),*]
                }

                fn call<'a>(
                    &'a self,
                    ctx: &'a ::gabion::ctx::Ctx,
                    op: &'a str,
                    input: ::gabion::serde_json::Value,
                ) -> ::gabion::ws::CallResult<'a> {
                    ::std::boxed::Box::pin(async move {
                        match op {
                            #(#call_arms)*
                            other => ::core::result::Result::Err(::gabion::error::WireError::new(
                                ::gabion::error::ErrorKind::NotFound,
                                format!("no op named `{other}` on {}", #ns_literal),
                            )),
                        }
                    })
                }

                fn open<'a>(
                    &'a self,
                    ctx: &'a ::gabion::ctx::Ctx,
                    op: &'a str,
                    input: ::gabion::serde_json::Value,
                ) -> ::gabion::ws::OpenResult<'a> {
                    ::std::boxed::Box::pin(async move {
                        match op {
                            #(#open_arms)*
                            other => ::core::result::Result::Err(::gabion::error::WireError::new(
                                ::gabion::error::ErrorKind::NotFound,
                                format!("`{other}` is not a listen op on {}", #ns_literal),
                            )),
                        }
                    })
                }
            }
        }
    }
}

/// One op and the policy its signature declared — what `x-gabion-streams` and the
/// open-stream registry both read.
fn op(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let name = tool_name(ns, m);
    let policy = match m.stream {
        Some(policy) => {
            let token = policy.token();
            quote! { ::core::option::Option::Some(#token) }
        }
        None => quote! { ::core::option::Option::None },
    };
    quote! {
        ::gabion::ws::Op {
            name: #name.to_string(),
            policy: #policy,
        }
    }
}

/// One ordinary op, dispatched as a `call` frame.
fn call_arm(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let name = tool_name(ns, m);
    let id = &m.ident;
    let parse = parse_input(ns, m);
    quote! {
        #name => {
            #parse
            match self.svc.#id(ctx, input).await {
                ::core::result::Result::Ok(v) => ::gabion::mcp::value_of(&v),
                ::core::result::Result::Err(e) => {
                    ::core::result::Result::Err(::gabion::mcp::wire_of(&e))
                }
            }
        }
    }
}

/// One listen op: start the stream, and report the policy the marker carried.
fn open_arm(ns: &str, m: &Method) -> Option<proc_macro2::TokenStream> {
    let policy = m.stream?;
    let name = tool_name(ns, m);
    let id = &m.ident;
    let parse = parse_input(ns, m);
    let token = policy.token();
    Some(quote! {
        #name => {
            #parse
            let started = self.svc.#id(ctx, input).await;
            match started {
                ::core::result::Result::Ok(marker) => ::core::result::Result::Ok(
                    ::gabion::ws::Opened {
                        policy: #token,
                        items: ::gabion::ws::encode_items(marker.into_stream()),
                    },
                ),
                ::core::result::Result::Err(e) => {
                    ::core::result::Result::Err(::gabion::mcp::wire_of(&e))
                }
            }
        }
    })
}

fn parse_input(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    match &m.input {
        Some(ty) => {
            let name = tool_name(ns, m);
            quote! {
                let input: #ty = match ::gabion::mcp::arguments(#name, input) {
                    ::core::result::Result::Ok(v) => v,
                    ::core::result::Result::Err(e) => return ::core::result::Result::Err(e),
                };
            }
        }
        None => quote! { let input = (); let _ = input; },
    }
}
