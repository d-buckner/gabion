//! The MCP surface: one tool per method, in the same order the trait declares
//! them, with the same name the HTTP route uses.
//!
//! The tool name is the route minus `/api`, computed by `tool_name`, so a client
//! reading the OpenAPI document and an agent reading `tools/list` are talking
//! about the same operation. The description is the method's doc comment, and the
//! input schema is built from the same type the route deserializes.

use quote::{format_ident, quote};

use crate::{tool_name, Contract, Method};

/// One generated MCP service plus the constructor a host mounts.
pub(crate) fn gen_mcp(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let base = &c.base;
    let trait_ident = &c.trait_ident;
    let ns_literal = ns.as_str();
    let mcp_mod = format_ident!("__{}_mcp", ns);
    let mcp_ident = format_ident!("{}Mcp", base);
    let mcp_fn = format_ident!("{}_mcp", ns);
    let tools = c.methods.iter().map(|m| tool(ns, m));
    let arms = c.methods.iter().map(|m| arm(ns, m));

    quote! {
        #[doc(hidden)]
        #[cfg(feature = "mcp")]
        pub mod #mcp_mod {
            #![allow(missing_docs, clippy::all, clippy::pedantic)]
            use super::*;

            /// One service as MCP sees it: the trait object plus the tool list
            /// generated from the trait.
            pub struct #mcp_ident {
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            }

            impl #mcp_ident {
                #[must_use]
                pub fn new(
                    svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
                ) -> Self {
                    Self { svc }
                }
            }

            /// The MCP surface for one impl. Mount it on a
            /// `gabion::mcp::Server` and every op in the trait becomes a tool —
            /// including the ones an agent will call unprompted.
            #[must_use]
            pub fn #mcp_fn(
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            ) -> #mcp_ident {
                #mcp_ident::new(svc)
            }

            impl ::gabion::mcp::Service for #mcp_ident {
                fn ns(&self) -> &str {
                    #ns_literal
                }

                fn tools(&self) -> ::std::vec::Vec<::gabion::mcp::Tool> {
                    let mut schemas = ::gabion::mcp::SchemaBuilder::new();
                    ::std::vec![#(#tools),*]
                }

                fn call<'a>(
                    &'a self,
                    ctx: &'a ::gabion::ctx::Ctx,
                    name: &'a str,
                    args: ::gabion::serde_json::Value,
                ) -> ::gabion::mcp::ToolResult<'a> {
                    ::std::boxed::Box::pin(async move {
                        match name {
                            #(#arms)*
                            other => ::core::result::Result::Err(::gabion::error::WireError::new(
                                ::gabion::error::ErrorKind::NotFound,
                                format!("no tool named `{other}` on {}", #ns_literal),
                            )),
                        }
                    })
                }
            }
        }
    }
}

/// The `tools/list` entry: name, prose from the doc comment, schema from the
/// input type.
fn tool(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let name = tool_name(ns, m);
    let description = describe(m);
    let schema = match &m.input {
        Some(ty) => quote! { schemas.value::<#ty>() },
        None => quote! { ::gabion::mcp::no_arguments() },
    };
    quote! {
        ::gabion::mcp::Tool {
            name: #name.to_string(),
            description: #description.to_string(),
            input_schema: #schema,
        }
    }
}

/// Summary and description joined: MCP has one description field, and a reviewer
/// who wrote two paragraphs meant both of them.
fn describe(m: &Method) -> String {
    if m.description.is_empty() {
        return m.summary.clone();
    }
    if m.summary.is_empty() {
        return m.description.clone();
    }
    format!("{}\n\n{}", m.summary, m.description)
}

/// One match arm: parse the arguments, call the impl, map the outcome onto the
/// MCP wire shapes.
fn arm(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let name = tool_name(ns, m);
    let id = &m.ident;
    let parse = match &m.input {
        Some(ty) => quote! {
            let input: #ty = match ::gabion::mcp::arguments(#name, args) {
                ::core::result::Result::Ok(v) => v,
                ::core::result::Result::Err(e) => return ::core::result::Result::Err(e),
            };
        },
        None => quote! { let _ = args; let input = (); },
    };
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
