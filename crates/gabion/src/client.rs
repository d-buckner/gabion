//! The generated typed client's mapping rules, in one place.
//!
//! A generated `<feature>Client<H>` implements the service trait over
//! [`Http`](crate::caps::Http): same methods, same wire types, same error type as
//! the in-process impl. The three things that can go wrong on the way — the
//! request never left, the reply was not JSON, the reply was a status — are
//! mapped here rather than in every generated method, so the mapping cannot
//! drift per service.
//!
//! The error type stays the app's: [`FromWire`](crate::error::FromWire) is what
//! turns a flat [`WireError`] back into the enum the contract declares, and the
//! app says once what an unrecognised remote failure becomes.

use serde::de::DeserializeOwned;

use crate::error::{ErrorKind, FromWire, WireError};

/// The request never became a response: DNS, TLS, connection refused, a
/// client-side deadline. A 404 is not this — it is a reply, and it has a kind.
///
/// `Err` is the generated method's declared error type, which is
/// `ServiceError<E>`; the `FromWire` impl on `ServiceError` supplies the typed
/// detail.
#[must_use]
pub fn transport<Err: FromWire>(detail: impl Into<String>) -> Err {
    Err::from_wire(WireError::new(ErrorKind::Internal, detail))
}

/// A non-2xx reply. When the body is a `WireError` the server's own kind wins —
/// a server that says "conflict" must not be re-read as "internal" from the
/// status code. When it is not, the status is mapped and the body kept as text,
/// because a proxy's error page is not a typed failure.
#[must_use]
pub fn from_failure<Err: FromWire>(status: u16, body: &[u8]) -> Err {
    if let Ok(wire) = serde_json::from_slice::<WireError>(body) {
        return Err::from_wire(wire);
    }
    let text = String::from_utf8_lossy(body);
    Err::from_wire(WireError::new(
        kind_for_status(status),
        format!("status {status}: {text}"),
    ))
}

/// A 2xx reply whose body is not the declared output type. The call succeeded and
/// the answer is unusable, which is the same situation as a store that returned
/// garbage: `Internal`.
pub fn from_success<T: DeserializeOwned, Err: FromWire>(
    status: u16,
    body: &[u8],
) -> Result<T, Err> {
    serde_json::from_slice(body).map_err(|e| {
        Err::from_wire(WireError::new(
            ErrorKind::Internal,
            format!("status {status} body did not parse: {e}"),
        ))
    })
}

/// The status-to-kind table the server side already agrees on. Kinds are
/// enumerated, so an unmapped status is `Internal` rather than an invention.
#[must_use]
pub fn kind_for_status(status: u16) -> ErrorKind {
    match status {
        400 | 422 => ErrorKind::InvalidInput,
        403 => ErrorKind::Forbidden,
        404 => ErrorKind::NotFound,
        408 => ErrorKind::Timeout,
        409 => ErrorKind::Conflict,
        _ => ErrorKind::Internal,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the mapping that broke
mod tests {
    use super::*;
    use crate::caps::HttpError;
    use crate::error::ServiceError;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Demo {
        NotFound,
        /// What an app keeps when the detail does not parse: the kind and message
        /// always survive, the typed variant cannot.
        Remote {
            kind: ErrorKind,
            message: String,
        },
    }

    impl FromWire for Demo {
        fn from_wire(wire: WireError) -> Self {
            Demo::Remote {
                kind: wire.kind,
                message: wire.message,
            }
        }
    }

    type DemoErr = ServiceError<Demo>;

    #[test]
    fn a_server_kind_beats_the_status_code() {
        let wire = WireError::new(ErrorKind::Conflict, "already done");
        let body = serde_json::to_vec(&wire).expect("wire");
        let err: DemoErr = from_failure(500, &body);
        assert_eq!(
            err.kind,
            ErrorKind::Conflict,
            "the server said conflict; a 500 must not relabel it"
        );
        assert_eq!(
            err.detail,
            Demo::Remote {
                kind: ErrorKind::Conflict,
                message: "already done".into()
            }
        );
    }

    /// A typed error whose detail this version knows is recovered whole — the
    /// client keeps the app's error model, not a lossy view of it.
    #[test]
    fn a_known_detail_comes_back_typed() {
        let typed: DemoErr =
            ServiceError::new(ErrorKind::NotFound, "todo not found", Demo::NotFound);
        let wire = WireError::from_service(&typed).expect("wire");
        let body = serde_json::to_vec(&wire).expect("wire");
        let back: DemoErr = from_failure(404, &body);
        assert_eq!(back.detail, Demo::NotFound);
        assert_eq!(back.kind, ErrorKind::NotFound);
    }

    #[test]
    fn a_body_that_is_not_a_wire_error_is_named_by_its_status() {
        let err: DemoErr = from_failure(404, b"<html>nope</html>");
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("<html>nope</html>"));
    }

    #[test]
    fn a_success_that_does_not_parse_is_internal_not_silent() {
        let outcome: Result<Vec<String>, DemoErr> = from_success(200, b"{\"not\":\"a list\"}");
        let err = outcome.expect_err("a body of the wrong shape must fail");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("did not parse"), "{}", err.message);
    }

    #[test]
    fn a_transport_failure_is_internal_and_keeps_the_reason() {
        let err: DemoErr = transport(format!(
            "transport: {}",
            HttpError::Transport("refused".into())
        ));
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("refused"), "{}", err.message);
    }

    #[test]
    fn the_status_table_is_the_one_the_server_uses() {
        for (status, kind) in [
            (400, ErrorKind::InvalidInput),
            (422, ErrorKind::InvalidInput),
            (403, ErrorKind::Forbidden),
            (404, ErrorKind::NotFound),
            (408, ErrorKind::Timeout),
            (409, ErrorKind::Conflict),
            (500, ErrorKind::Internal),
            (418, ErrorKind::Internal),
        ] {
            assert_eq!(kind_for_status(status), kind, "status {status}");
        }
    }
}
