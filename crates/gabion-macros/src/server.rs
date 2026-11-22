//! The HTTP surface: an axum router with one POST per method, and the OpenAPI
//! 3.1 document that describes exactly those routes.

use quote::{format_ident, quote};

use crate::{route, tool_name, Contract};

pub(crate) fn gen_server(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_ident = &c.trait_ident;
    let server_mod = format_ident!("__{}_server", ns);
    let router_fn = format_ident!("{}_router", ns);
    let handlers = gen_handlers(c);
    let routes = gen_routes(c);
    let openapi = gen_openapi(c);

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

            #handlers

            /// One POST per method: `POST /<ns>.<method>`.
            pub fn #router_fn(
                svc: ::std::sync::Arc<dyn #trait_ident + ::core::marker::Send + ::core::marker::Sync>,
            ) -> ::axum::Router {
                ::axum::Router::new()
                    #routes
                    .with_state(Svc(svc))
            }

            #openapi
        }
    }
}

/// One axum handler per method. Each reads the caller from its headers before it
/// looks at the body, because a route that parses first and identifies later
/// gives an anonymous caller a parser.
///
/// A listen op gets none: one JSON body cannot carry N items, and a route that
/// returned them one at a time would be a second, worse WebSocket. The op is
/// served on `/api/ws` instead, and named in `x-gabion-streams`.
fn gen_handlers(c: &Contract) -> proc_macro2::TokenStream {
    let handlers = c.methods.iter().filter(|m| m.has_post_route()).map(|m| {
        let id = &m.ident;
        let hid = format_ident!("__h_{}", m.snake);
        let parse = match &m.input {
            Some(ty) => quote! {
                let input: #ty = match ::gabion::serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(err) => {
                        // Same envelope as a service error, so a client parses
                        // one shape for every failure.
                        let wire = ::gabion::error::WireError::new(
                            ::gabion::error::ErrorKind::InvalidInput,
                            err.to_string(),
                        );
                        return (
                            ::axum::http::StatusCode::BAD_REQUEST,
                            ::axum::Json(wire),
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
                headers: ::axum::http::HeaderMap,
                body: ::axum::body::Bytes,
            ) -> ::axum::response::Response {
                // The caller is not invented here: the headers the generated
                // client stamps are the headers this reads.
                let ctx = ::gabion::ctx::Ctx::from_wire_headers(
                    headers.iter().filter_map(|(name, value)| {
                        value.to_str().ok().map(|v| (name.as_str(), v))
                    }),
                );
                #parse
                match svc.#id(&ctx, input).await {
                    Ok(v) => (::axum::http::StatusCode::OK, ::axum::Json(&v)).into_response(),
                    Err(e) => {
                        let status = ::axum::http::StatusCode::from_u16(e.kind.http_status())
                            .unwrap_or(::axum::http::StatusCode::INTERNAL_SERVER_ERROR);
                        // A detail that will not serialize must not swallow the
                        // error: send the kind and message, which is all a
                        // client is guaranteed to understand anyway.
                        let wire = ::gabion::error::WireError::from_service(&e)
                            .unwrap_or_else(|_| {
                                ::gabion::error::WireError::new(
                                    ::gabion::error::ErrorKind::Internal,
                                    "error detail failed to serialize",
                                )
                            });
                        (status, ::axum::Json(wire)).into_response()
                    }
                }
            }
        }
    });
    quote! { #(#handlers)* }
}

fn gen_routes(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let routes = c.methods.iter().filter(|m| m.has_post_route()).map(|m| {
        let hid = format_ident!("__h_{}", m.snake);
        let route = route(ns, m);
        quote! { .route(#route, ::axum::routing::post(#hid)) }
    });
    quote! { #(#routes)* }
}

/// The OpenAPI 3.1 document for the routes above, built from the same types the
/// handlers deserialize. Gated on the router feature so a document can never
/// describe a route that is not served.
fn gen_openapi(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_name = &c.trait_name;
    let openapi_fn = format_ident!("{}_openapi", ns);
    let methods = &c.methods;

    let paths = methods.iter().filter(|m| m.has_post_route()).map(|m| {
        let path = route(ns, m);
        let operation_id = format!("{}_{}", ns, m.snake);
        let summary = &m.summary;
        let description = &m.description;
        let output = &m.output;
        let request = match &m.input {
            Some(ty) => quote! { Some(generator.subschema_for::<#ty>()) },
            None => quote! { None },
        };
        quote! {
            let __request: Option<::schemars::Schema> = #request;
            let __response = generator.subschema_for::<#output>();
            doc.post(::gabion::openapi::Route {
                path: #path,
                operation_id: #operation_id,
                summary: #summary,
                description: #description,
                request: __request,
                response: __response,
            });
        }
    });

    let streams = methods.iter().filter_map(|m| {
        let policy = m.stream?;
        let op = tool_name(ns, m);
        let output = &m.output;
        let policy = policy.as_str();
        Some(quote! {
            let __item: ::schemars::Schema = generator.subschema_for::<#output>();
            doc.stream(#op, #policy, __item);
        })
    });

    quote! {
        /// OpenAPI 3.1 for this service, generated from the trait and the types
        /// the router already uses. Commit the output; `gabion check` runs the
        /// test that fails when the document and the code disagree.
        #[cfg(feature = "openapi")]
        pub fn #openapi_fn() -> ::gabion::serde_json::Value {
            let mut generator = ::schemars::generate::SchemaSettings::draft2020_12()
                // OpenAPI 3.1 schemas are JSON Schema 2020-12, but they live under
                // `components/schemas`, so the refs must point there and the
                // per-schema `$schema` goes.
                .with(|s| {
                    s.meta_schema = None;
                    s.definitions_path = "/components/schemas".into();
                })
                .into_generator();
            let mut doc = ::gabion::openapi::OpenApi::new(
                #trait_name,
                env!("CARGO_PKG_VERSION"),
                #ns,
            );
            doc.error_response(generator.subschema_for::<::gabion::error::WireError>());
            #(#paths)*
            #(#streams)*
            doc.finish(generator.take_definitions(true))
        }
    }
}
