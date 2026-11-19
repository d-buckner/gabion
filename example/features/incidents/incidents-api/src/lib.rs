//! incidents feature contract: an outage, opened and closed from what the
//! watches saw.
//!
//! This crate depends on `checks-api` and nothing from `checks-core`. Everything
//! this feature knows about a probe it learned through the checks contract, which
//! is the same door an HTTP client walks through.

use checks_api::CheckId;
use gabion::prelude::*;

/// One outage of one watch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Incident {
    pub id: String,
    pub check_id: String,
    /// Copied at open time: a watch may be renamed or removed while its outage
    /// is still on the record.
    pub check_name: String,
    pub opened_at: u64,
    /// `None` while the outage is running.
    pub closed_at: Option<u64>,
    /// What the probes said, kept from the probe that opened this.
    pub cause: String,
}

/// How much of a watch's recent history came back.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Uptime {
    pub check_id: String,
    /// Probes counted, which is the whole history up to the window cap.
    pub probes: u32,
    pub ok: u32,
    /// Whole percent, rounded down. `100` only when every probe in the window
    /// came back, so a watch with no history is `0`, not `100`.
    pub pct: u8,
}

/// One error enum -> HTTP status, CLI exit code, through `ErrorKind`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum IncidentError {
    NotFound,
    /// A store, clock, or neighbor-contract failure: the request was fine, the
    /// world was not. Maps to 500 / exit 1 via `ErrorKind`, never hand-mapped.
    Internal,
}

impl IncidentError {
    fn kind(&self) -> ErrorKind {
        match self {
            IncidentError::NotFound => ErrorKind::NotFound,
            IncidentError::Internal => ErrorKind::Internal,
        }
    }
}

pub type IncidentErr = ServiceError<IncidentError>;

/// The public contract. **Every method in this trait is generated into every
/// surface** — CLI subcommand, HTTP route, OpenAPI operation — with no per-method
/// opt-in or opt-out. The trait is the security boundary: adding a method here
/// grants it to every caller, including agents.
#[service]
pub trait IncidentService {
    /// Read what the watches saw and open or close incidents to match. Returns
    /// the incidents this call changed.
    async fn evaluate(&self, _input: ()) -> Result<Vec<Incident>, IncidentErr>;

    /// Every incident, open ones first, newest first within each group.
    async fn list(&self, _input: ()) -> Result<Vec<Incident>, IncidentErr>;

    /// Share of one watch's recent probes that came back.
    async fn uptime(&self, input: CheckId) -> Result<Uptime, IncidentErr>;
}

/// Constructor helper: builds the wire envelope from a domain error.
pub fn incident_err(detail: IncidentError) -> IncidentErr {
    let kind = detail.kind();
    let msg = match &detail {
        IncidentError::NotFound => "incident not found".to_string(),
        IncidentError::Internal => "storage failed".to_string(),
    };
    ServiceError::new(kind, msg, detail)
}
