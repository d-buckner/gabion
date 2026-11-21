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

use crate::{route, Contract, Method};

/// The remote impl. Feature-gated on `client`, which is on by default: a contract
/// with no client is a contract a wasm renderer cannot call.
pub(crate) fn gen_client(c: &Contract) -> proc_macro2::TokenStream {
    let base = &c.base;
    let trait_ident = &c.trait_ident;
    let client_ident = format_ident!("{}Client", base);
    let methods = c.methods.iter().map(|m| method(&c.ns, m));
    let async_trait = crate::async_trait_attrs();

    quote! {
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
    }
}

/// One method, forwarded as a POST with the caller's headers attached.
fn method(ns: &str, m: &Method) -> proc_macro2::TokenStream {
    let id = &m.ident;
    let path = route(ns, m);
    let output = &m.output;
    let error = &m.error;
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
