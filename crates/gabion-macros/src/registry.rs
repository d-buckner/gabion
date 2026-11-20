//! Registration: the inherent method `#[service]` puts on the trait object, so a
//! host mounts an impl and gets every generated surface with it (spec §4).
//!
//! ```ignore
//! let mut host = Host::new();
//! checks.mount_on(&mut host);       // routes, subcommand, tools — all of them
//! incidents.mount_on(&mut host);
//! ```
//!
//! The host names the impl and nothing else: not the namespace, not the router
//! function, not the tool list. `gabion::registry` explains why this is an
//! inherent method rather than a linker-collected slice: the slice version is
//! silently empty in an rlib build, and a registry that quietly misses a contract
//! is the failure class this framework exists to remove.
//!
//! Each surface is behind its own cargo feature, so a host that enables only
//! `server` gets a registration whose other fields are `None` rather than a
//! compile error.

use quote::{format_ident, quote};

use crate::{route, tool_name, Contract};

pub(crate) fn gen_registry(c: &Contract) -> proc_macro2::TokenStream {
    let ns = &c.ns;
    let trait_name = &c.trait_name;
    let trait_ident = &c.trait_ident;
    let ns_literal = ns.as_str();
    let mcp_mod = format_ident!("__{}_mcp", ns);
    let mcp_fn = format_ident!("{}_mcp", ns);
    let server_mod = format_ident!("__{}_server", ns);
    let router_fn = format_ident!("{}_router", ns);
    let cli_mod = format_ident!("__{}_cli", ns);
    let cli_fn = format_ident!("{}_cli_command", ns);

    let ops: Vec<String> = c.methods.iter().map(|m| tool_name(ns, m)).collect();
    let routes: Vec<String> = c.methods.iter().map(|m| route(ns, m)).collect();

    quote! {
        #[cfg(feature = "registry")]
        impl dyn #trait_ident + Send + Sync {
            /// Mount this impl on a host: the routes, the subcommand and the tools
            /// this contract generates, with the op list a host can log.
            ///
            /// A host calls this on the `Arc<dyn Trait>` it already holds, so it
            /// never names a surface. Adding an op to the trait adds it to every
            /// host that mounts it, which is the property the whole design claims.
            pub fn mount_on(self: ::std::sync::Arc<Self>, host: &mut ::gabion::registry::Host) {
                host.mount(::gabion::registry::Surfaces {
                    ns: #ns_literal,
                    trait_name: #trait_name,
                    ops: &[#(#ops),*],
                    routes: &[#(#routes),*],
                    #[cfg(feature = "server")]
                    router: ::core::option::Option::Some(::std::boxed::Box::new(
                        #server_mod::#router_fn(self.clone()),
                    )),
                    #[cfg(not(feature = "server"))]
                    router: ::core::option::Option::None,
                    #[cfg(feature = "cli")]
                    command: ::core::option::Option::Some(::std::boxed::Box::new(
                        #cli_mod::#cli_fn(),
                    )),
                    #[cfg(not(feature = "cli"))]
                    command: ::core::option::Option::None,
                    #[cfg(feature = "mcp")]
                    mcp: ::core::option::Option::Some(::std::sync::Arc::new(#mcp_mod::#mcp_fn(
                        self,
                    )) as ::std::sync::Arc<dyn ::gabion::mcp::Service>),
                    #[cfg(not(feature = "mcp"))]
                    mcp: ::core::option::Option::None,
                });
            }
        }
    }
}
