//! The one error model: a service error carries an HTTP status, an MCP error
//! code, and a CLI exit code — derived from one enum, never hand-mapped per
//! adapter.
//!
//! Two types, on purpose. `ServiceError<E>` is the in-process error: rich, typed
//! by the app's own enum, and never serialized directly. `WireError` is what
//! crosses a surface: flat, bidirectional, and independent of the app's error
//! type so a client on a different version of the contract can still read the
//! kind. Converting in-proc -> wire keeps the kind; converting back recovers the
//! typed error when the client knows it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Error kinds map 1:1 onto transport-level outcomes. Serialized by name, so
/// the kind survives a round trip instead of being dropped at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

impl ErrorKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "not_found",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Internal => "internal",
        }
    }
    #[must_use]
    pub fn http_status(self) -> u16 {
        match self {
            ErrorKind::NotFound => 404,
            ErrorKind::InvalidInput => 400,
            ErrorKind::Conflict => 409,
            ErrorKind::Internal => 500,
        }
    }
    /// JSON-RPC-style code used for MCP errors.
    #[must_use]
    pub fn mcp_code(self) -> i32 {
        match self {
            ErrorKind::NotFound => -32001,
            ErrorKind::InvalidInput => -32602,
            ErrorKind::Conflict => -32002,
            ErrorKind::Internal => -32603,
        }
    }
    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::NotFound => 3,
            ErrorKind::InvalidInput => 2,
            ErrorKind::Conflict => 4,
            ErrorKind::Internal => 1,
        }
    }
    /// The inverse of [`ErrorKind::exit_code`], for a caller that only has a
    /// process status. `None` for a code no surface produces: 0 is success, and
    /// `2` is deliberately `InvalidInput` because that is what clap uses for a
    /// usage error too.
    #[must_use]
    pub fn from_exit_code(code: i32) -> Option<Self> {
        match code {
            1 => Some(ErrorKind::Internal),
            2 => Some(ErrorKind::InvalidInput),
            3 => Some(ErrorKind::NotFound),
            4 => Some(ErrorKind::Conflict),
            _ => None,
        }
    }
}

/// The in-process error: `E` is the app's own error enum, so nothing is lost
/// while the call stays in the process. Not serialized directly — see
/// [`WireError`].
#[derive(Debug)]
pub struct ServiceError<E> {
    pub kind: ErrorKind,
    pub message: String,
    pub detail: E,
}

impl<E> ServiceError<E> {
    pub fn new(kind: ErrorKind, message: impl Into<String>, detail: E) -> Self {
        Self {
            kind,
            message: message.into(),
            detail,
        }
    }

    /// Rewrite the payload while keeping the transport mapping.
    pub fn map_detail<T>(self, f: impl FnOnce(E) -> T) -> ServiceError<T> {
        ServiceError {
            kind: self.kind,
            message: self.message,
            detail: f(self.detail),
        }
    }
}

impl<E> std::fmt::Display for ServiceError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for ServiceError<E> {}

/// The flat, bidirectional wire form of a failed call. `detail` is structural
/// JSON rather than the app's error type, so an older or newer client still
/// gets `kind` and `message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WireError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}

impl WireError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: serde_json::Value::Null,
        }
    }

    /// Project an in-proc error onto the wire. Fallible because `E: Serialize`
    /// can still fail (a custom impl, a float that is not finite); a caller that
    /// cannot serialize the detail should send the kind and message alone
    /// rather than dropping the whole error.
    pub fn from_service<E: Serialize>(e: &ServiceError<E>) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: e.kind,
            message: e.message.clone(),
            detail: serde_json::to_value(&e.detail)?,
        })
    }

    /// Reconstruct the typed error, when the caller knows `E`. On failure the
    /// original wire error comes back unchanged — the kind and message are
    /// always usable.
    pub fn into_service_error<E: serde::de::DeserializeOwned>(
        self,
    ) -> Result<ServiceError<E>, Self> {
        match serde_json::from_value(self.detail.clone()) {
            Ok(detail) => Ok(ServiceError {
                kind: self.kind,
                message: self.message,
                detail,
            }),
            Err(_) => Err(self),
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for WireError {}

/// A type that can be rebuilt from a failure that crossed a wire.
///
/// The generated client needs this: a remote call comes back as a flat
/// [`WireError`], and the client's signature promises the app's own error type.
/// When the detail on the wire is the shape this version knows, recovery is
/// total. When it is not — an older server, a newer server, a transport failure
/// that never had a detail at all — the app still has to answer with *something*
/// typed, and "something" is a decision only the app can make. So it makes it,
/// once, in an impl the compiler will demand the moment a client is generated.
///
/// ```ignore
/// impl gabion::error::FromWire for TodoError {
///     fn from_wire(wire: WireError) -> Self {
///         TodoError::Remote { kind: wire.kind, message: wire.message }
///     }
/// }
/// ```
pub trait FromWire: Sized {
    /// Rebuild the app's error from whatever did arrive.
    fn from_wire(wire: WireError) -> Self;
}

/// `ServiceError<E>` is recoverable whenever `E` is: the typed detail is kept
/// when it parses, and `E::from_wire` supplies the fallback when it does not.
impl<E: FromWire + serde::de::DeserializeOwned> FromWire for ServiceError<E> {
    fn from_wire(wire: WireError) -> Self {
        match wire.into_service_error::<E>() {
            Ok(typed) => typed,
            Err(leftover) => ServiceError {
                kind: leftover.kind,
                message: leftover.message.clone(),
                detail: E::from_wire(leftover),
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should say which conversion broke
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
    enum DemoError {
        NotFound,
        Weird { field: u8 },
    }

    fn demo() -> ServiceError<DemoError> {
        ServiceError::new(ErrorKind::NotFound, "todo not found", DemoError::NotFound)
    }

    #[test]
    fn kind_survives_the_wire() {
        let json = serde_json::to_string(&WireError::from_service(&demo()).expect("wire"))
            .expect("serialize");
        let back: WireError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.kind, ErrorKind::NotFound);
        assert_eq!(back.message, "todo not found");
    }

    #[test]
    fn typed_error_is_recoverable_by_a_client_that_knows_it() {
        let wire = WireError::from_service(&demo()).expect("wire");
        let typed: ServiceError<DemoError> = wire.into_service_error().expect("typed");
        assert_eq!(typed.detail, DemoError::NotFound);
        assert_eq!(typed.kind, ErrorKind::NotFound);
    }

    #[test]
    fn a_client_that_does_not_know_the_detail_still_gets_kind_and_message() {
        #[derive(Debug, Serialize, serde::Deserialize)]
        struct Other;
        let wire = WireError::from_service(&demo()).expect("wire");
        let leftover = wire
            .clone()
            .into_service_error::<Other>()
            .expect_err("unknown detail");
        assert_eq!(leftover.kind, ErrorKind::NotFound);
        assert_eq!(leftover.message, "todo not found");
    }

    #[test]
    fn structured_detail_round_trips() {
        let rich = ServiceError::new(
            ErrorKind::Conflict,
            "already done",
            DemoError::Weird { field: 7 },
        );
        let wire = WireError::from_service(&rich).expect("wire");
        let typed: ServiceError<DemoError> = wire.into_service_error().expect("typed");
        assert_eq!(typed.detail, DemoError::Weird { field: 7 });
    }

    #[test]
    fn one_kind_maps_to_every_surface() {
        assert_eq!(ErrorKind::NotFound.http_status(), 404);
        assert_eq!(ErrorKind::NotFound.mcp_code(), -32001);
        assert_eq!(ErrorKind::NotFound.exit_code(), 3);
        assert_eq!(ErrorKind::Internal.exit_code(), 1);
    }

    /// Every mapping round-trips, so a surface that only has a status can recover
    /// the kind. A code that no surface produces is `None`, not `Internal`.
    #[test]
    fn every_exit_code_maps_back_to_its_kind() {
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::InvalidInput,
            ErrorKind::Conflict,
            ErrorKind::Internal,
        ] {
            assert_eq!(ErrorKind::from_exit_code(kind.exit_code()), Some(kind));
        }
        assert_eq!(ErrorKind::from_exit_code(0), None);
        assert_eq!(ErrorKind::from_exit_code(99), None);
    }

    #[test]
    fn display_names_the_kind() {
        assert_eq!(demo().to_string(), "not_found: todo not found");
        assert_eq!(
            WireError::new(ErrorKind::Conflict, "nope").to_string(),
            "conflict: nope"
        );
    }
}
