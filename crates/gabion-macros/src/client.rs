//! The typed client: the service trait implemented over `Http`.
//!
//! The generated `<Feature>Client<H>` implements the *service trait itself*, not
//! a parallel client trait. That is deliberate. A renderer takes
//! `Arc<dyn TodoService>` from its host; whether the host handed it the core or a
//! remote client is not the screen's business, and two traits would mean two
//! signatures to keep in step — a drift no test would catch.
//!
//! The consequence is that the in-process surface needs no code: the trait object
//! the host already has *is* the in-process client. Only the remote one is
//! generated, and it travels on the `Http` capability, so the same client works
//! in a browser (`fetch`) and on a server (reqwest) with no change.

use quote::{format_ident, quote};

use crate::{route, tool_name, Contract, Method, Policy};

/// The remote impl. Feature-gated on `client`, which is on by default: a contract
/// with no client is a contract a wasm renderer cannot call.
pub(crate) fn gen_client(c: &Contract) -> proc_macro2::TokenStream {
    let base = &c.base;
    let trait_ident = &c.trait_ident;
    let client_ident = format_ident!("{}Client", base);
    let streams = c.streams();
    let methods = c.methods.iter().map(|m| method(&c.ns, m, streams));
    let async_trait = crate::async_trait_attrs();

    // A feature that streams needs a socket as well as `Http`, so the client gains
    // a second capability parameter. It is added only when something streams, and
    // `new` stays in a one-parameter impl, so a host that never streams writes
    // exactly what it wrote before.
    if !c.streams() {
        return quote! {
            /// The service trait, implemented over `Http`: the same methods, the same
            /// wire types, and the same error type as the in-process impl.
            ///
            /// `base` is the other process's root (`http://127.0.0.1:3456`); each call
            /// POSTs to `{base}/api/<ns>.<method>`. The caller's `Ctx` travels as
            /// headers, so an identity set here is the identity the other end reads.
            #[cfg(feature = "client")]
            pub struct #client_ident<H: ::gabion::caps::Http> {
                base: ::std::string::String,
                http: H,
            }

            #[cfg(feature = "client")]
            impl<H: ::gabion::caps::Http> #client_ident<H> {
                #[must_use]
                pub fn new(base: impl ::core::convert::Into<::std::string::String>, http: H) -> Self {
                    Self {
                        // A trailing slash in the base would produce `//api/...`,
                        // which some proxies 404 and nobody enjoys debugging.
                        base: base.into().trim_end_matches('/').to_string(),
                        http,
                    }
                }

                #[must_use]
                pub fn base(&self) -> &str {
                    &self.base
                }
            }

            #[cfg(feature = "client")]
            #(#async_trait)*
            impl<H: ::gabion::caps::Http> #trait_ident for #client_ident<H> {
                #(#methods)*
            }
        };
    }

    quote! {
        /// The service trait, implemented over `Http` and, for its listen ops, over
        /// the host's `Streams` capability. Same methods, same wire types, same
        /// error type as the in-process impl.
        ///
        /// One `/api/ws` connection serves the whole feature: a listen op opens a
        /// stream on it, and the feature's other ops travel over it as `call` frames
        /// once that stream is open (§5).
        #[cfg(feature = "client")]
        pub struct #client_ident<H: ::gabion::caps::Http, S: ::gabion::caps::Streams> {
            base: ::std::string::String,
            http: H,
            conn: ::gabion::ws::Connection<S>,
        }

        #[cfg(feature = "client")]
        impl<H: ::gabion::caps::Http, S: ::gabion::caps::Streams> #client_ident<H, S> {
            #[must_use]
            pub fn base(&self) -> &str {
                &self.base
            }

            /// The client that can stream: `Http` for the ordinary calls made
            /// before any stream is open, and the capability that dials `/api/ws`.
            #[must_use]
            pub fn with_streams(
                base: impl ::core::convert::Into<::std::string::String>,
                http: H,
                streams: S,
            ) -> Self {
                let base = base.into().trim_end_matches('/').to_string();
                let conn = ::gabion::ws::Connection::new(format!("{base}/api/ws"), streams);
                Self {
                    base,
                    http,
                    conn,
                }
            }
        }

        /// A client built without a socket still compiles, and its listen ops fail
        /// by name rather than being missing: a contract cannot offer a host an op
        /// it has no way to reach.
        #[cfg(feature = "client")]
        impl<H: ::gabion::caps::Http> #client_ident<H, ::gabion::caps::NoStreams> {
            #[must_use]
            pub fn new(base: impl ::core::convert::Into<::std::string::String>, http: H) -> Self {
                Self::with_streams(base, http, ::gabion::caps::NoStreams)
            }
        }

        #[cfg(feature = "client")]
        #(#async_trait)*
        impl<H: ::gabion::caps::Http, S: ::gabion::caps::Streams> #trait_ident
            for #client_ident<H, S>
        {
            #(#methods)*
        }
    }
}

/// One method. A listen op opens a stream on the connection and decodes `item`
/// frames. An ordinary op is a POST, except in a feature that streams and has a
/// stream open, where it is a `call` frame on that connection (§5).
fn method(ns: &str, m: &Method, streams: bool) -> proc_macro2::TokenStream {
    match m.stream {
        Some(policy) => stream_method(ns, m, policy),
        None => http_method(ns, m, streams),
    }
}

/// A listen op: open on `/api/ws`, then decode. The `call` frames of the same
/// namespace travel on the same connection (§5), which is why the client holds one
/// connection rather than one per op.
fn stream_method(ns: &str, m: &Method, policy: Policy) -> proc_macro2::TokenStream {
    let id = &m.ident;
    let op = tool_name(ns, m);
    let output = &m.output;
    let error = &m.error;
    let marker = match policy {
        Policy::Each => quote! { ::gabion::stream::Each },
        Policy::Latest => quote! { ::gabion::stream::Latest },
    };
    let (param, encode) = input_encoding(m);
    quote! {
        async fn #id(
            &self,
            ctx: &::gabion::ctx::Ctx,
            #param,
        ) -> ::core::result::Result<#marker<#output>, #error> {
            #encode
            let items = match self
                .conn
                .stream::<#output, #error>(#op, encoded, ctx)
                .await
            {
                ::core::result::Result::Ok(v) => v,
                ::core::result::Result::Err(wire) => {
                    return ::core::result::Result::Err(<#error as ::gabion::error::FromWire>::from_wire(wire))
                }
            };
            ::core::result::Result::Ok(#marker::new(::gabion::ws::decode_items::<#output, #error>(items)))
        }
    }
}

/// The `input` parameter and the code that turns it into bytes or a `Value`.
fn input_encoding(m: &Method) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let error_ty = &m.error;
    match &m.input {
        Some(ty) => (
            quote! { input: #ty },
            quote! {
                let encoded = match ::gabion::serde_json::to_value(&input) {
                    Ok(v) => v,
                    Err(e) => {
                        return ::core::result::Result::Err(::gabion::client::transport::<#error_ty>(
                            format!("input did not serialize: {e}"),
                        ))
                    }
                };
            },
        ),
        None => (
            quote! { _input: () },
            quote! { let encoded = ::gabion::serde_json::Value::Null; },
        ),
    }
}

/// The `call`-frame branch of an ordinary op in a feature that streams.
///
/// §5: a feature that streams puts its ordinary ops on the connection too. Not
/// always — the wall only lets a `call` into a namespace with a stream open, so
/// before the first stream this op is still a POST.
fn socket_branch(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let op = tool_name(ns, m);
    let error = &m.error;
    let frame_input = match &m.input {
        Some(_) => quote! {
            match ::gabion::serde_json::to_value(&input) {
                Ok(v) => v,
                Err(e) => {
                    return Err(::gabion::client::transport::<#error>(
                        format!("input did not serialize: {e}"),
                    ))
                }
            }
        },
        None => quote! { ::gabion::serde_json::Value::Null },
    };
    quote! {
        if self.conn.has_open_stream(#ns) {
            let encoded = #frame_input;
            return match self.conn.call(#op, encoded, ctx).await {
                ::core::result::Result::Ok(value) => {
                    ::gabion::serde_json::from_value(value).map_err(|e| {
                        ::gabion::client::transport::<#error>(format!(
                            "reply would not deserialize: {e}"
                        ))
                    })
                }
                ::core::result::Result::Err(wire) => {
                    Err(<#error as ::gabion::error::FromWire>::from_wire(wire))
                }
            };
        }
    }
}
fn http_method(ns: &str, m: &Method, streams: bool) -> proc_macro2::TokenStream {
    let id = &m.ident;
    let path = route(ns, m);
    let output = &m.output;
    let error = &m.error;
    let on_socket = streams.then(|| socket_branch(ns, m));
    let (param, encode) = match &m.input {
        Some(ty) => (
            quote! { input: #ty },
            quote! {
                let encoded = match ::gabion::serde_json::to_vec(&input) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(::gabion::client::transport::<#error>(
                            format!("input did not serialize: {e}"),
                        ))
                    }
                };
            },
        ),
        None => (
            quote! { _input: () },
            quote! { let encoded: ::std::vec::Vec<u8> = ::std::vec::Vec::new(); },
        ),
    };
    quote! {
        async fn #id(
            &self,
            ctx: &::gabion::ctx::Ctx,
            #param,
        ) -> ::core::result::Result<#output, #error> {
            #on_socket
            #encode
            let mut request = ::gabion::caps::HttpRequest::post(
                format!("{}{}", self.base, #path),
                encoded,
            )
            .with_header("content-type", "application/json");
            for (name, value) in ctx.wire_headers() {
                request = request.with_header(name, value);
            }
            let reply = match self.http.send(request).await {
                Ok(reply) => reply,
                Err(e) => return Err(::gabion::client::transport::<#error>(e.to_string())),
            };
            if !reply.is_success() {
                return Err(::gabion::client::from_failure::<#error>(
                    reply.status,
                    &reply.body,
                ));
            }
            ::gabion::client::from_success::<#output, #error>(reply.status, &reply.body)
        }
    }
}
