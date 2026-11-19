//! The one error model: a service error carries an HTTP status, an MCP error
//! code, and a CLI exit code — derived from one enum, never hand-mapped per adapter.

use serde::Serialize;

/// Error kinds map 1:1 onto transport-level outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

impl ErrorKind {
    pub fn http_status(self) -> u16 {
        match self {
            ErrorKind::NotFound => 404,
            ErrorKind::InvalidInput => 400,
            ErrorKind::Conflict => 409,
            ErrorKind::Internal => 500,
        }
    }
    /// JSON-RPC-style code used for MCP errors.
    pub fn mcp_code(self) -> i32 {
        match self {
            ErrorKind::NotFound => -32001,
            ErrorKind::InvalidInput => -32602,
            ErrorKind::Conflict => -32002,
            ErrorKind::Internal => -32603,
        }
    }
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::NotFound => 3,
            ErrorKind::InvalidInput => 2,
            ErrorKind::Conflict => 4,
            ErrorKind::Internal => 1,
        }
    }
}

/// Wire-visible error envelope. `E: Serialize` comes from the app's error enum.
#[derive(Debug, Serialize)]
pub struct ServiceError<E: Serialize> {
    pub kind: &'static str,
    pub message: String,
    #[serde(skip)]
    pub kind_code: ErrorKind,
    #[serde(flatten)]
    pub detail: E,
}

impl<E: Serialize> ServiceError<E> {
    pub fn new(kind: ErrorKind, message: impl Into<String>, detail: E) -> Self {
        Self {
            kind: match kind {
                ErrorKind::NotFound => "not_found",
                ErrorKind::InvalidInput => "invalid_input",
                ErrorKind::Conflict => "conflict",
                ErrorKind::Internal => "internal",
            },
            message: message.into(),
            kind_code: kind,
            detail,
        }
    }
}
