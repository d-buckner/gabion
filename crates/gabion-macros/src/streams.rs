//! The stream markers in a return type: what they mean, and what a malformed one
//! costs the author.
//!
//! The policy is read off `Each<T>` / `Latest<T>` rather than off an attribute
//! because `.gabion/surfaces.lock` hashes signatures and does not hash attributes.
//! A policy carried by an attribute could flip and move behaviour without moving
//! the lock, which is the whole reason the marker is a type.

use quote::quote;
use syn::Type;

use crate::is_streaming;

/// The flow policy a return marker declared.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    Each,
    Latest,
}

impl Policy {
    /// The name `x-gabion-streams` records for this policy.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Policy::Each => "each",
            Policy::Latest => "latest",
        }
    }

    /// The token the generated surface names for this policy.
    pub(crate) fn token(self) -> proc_macro2::TokenStream {
        match self {
            Policy::Each => quote! { ::gabion::ws::Policy::Each },
            Policy::Latest => quote! { ::gabion::ws::Policy::Latest },
        }
    }
}

/// What a declared `Ok` type says about streaming.
enum Marker {
    /// An ordinary value.
    Plain(Type),
    /// `Each<T>` or `Latest<T>`, with the item type inside.
    Streamed(Policy, Type),
    /// Something shaped like a marker that is not one of the two.
    Malformed(Type),
}

/// Read the return type: an ordinary value, or a stream marker and its item type.
///
/// Only the last path segment is compared, so `Each<T>`, `stream::Each<T>` and
/// `gabion::stream::Each<T>` all read the same (docs/streaming.md §4.3).
pub(crate) fn read_return(declared: &Type) -> syn::Result<(Type, Option<Policy>)> {
    match marker(declared) {
        Marker::Plain(plain) => {
            if is_streaming(&plain) {
                return Err(syn::Error::new_spanned(&plain, NO_UNMARKED_STREAM));
            }
            Ok((plain, None))
        }
        Marker::Streamed(policy, inner) => Ok((inner, Some(policy))),
        Marker::Malformed(ty) => Err(syn::Error::new_spanned(&ty, MALFORMED_MARKER)),
    }
}

fn marker(declared: &Type) -> Marker {
    let Type::Path(path) = declared else {
        return Marker::Plain(declared.clone());
    };
    let Some(segment) = path.path.segments.last() else {
        return Marker::Plain(declared.clone());
    };
    let policy = match segment.ident.to_string().as_str() {
        "Each" => Policy::Each,
        "Latest" => Policy::Latest,
        _ => return Marker::Plain(declared.clone()),
    };
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Marker::Malformed(declared.clone());
    };
    match args.args.first() {
        Some(syn::GenericArgument::Type(inner)) => Marker::Streamed(policy, inner.clone()),
        _ => Marker::Malformed(declared.clone()),
    }
}

/// The message for a stream return that names no policy. The policy is the
/// marker, so an unmarked stream has no policy to run — and no default exists,
/// because dropping items and delivering every item are different promises.
const NO_UNMARKED_STREAM: &str = "an unmarked stream return is refused: name the policy with \
                                  `Each<T>` (every item, or the subscriber waits) or `Latest<T>` \
                                  (the newest, and an untaken one is replaced). The policy selects \
                                  the loop every surface emits, so a stream without a marker is a \
                                  stream whose flow rules nobody chose.";

/// A marker with nothing, or a non-type, inside it.
const MALFORMED_MARKER: &str = "`Each<T>` and `Latest<T>` take the item type: `Each<Reading>`, \
                                `Latest<Frame>`. Nothing inside the marker means nothing to decode \
                                an `item` frame into.";
