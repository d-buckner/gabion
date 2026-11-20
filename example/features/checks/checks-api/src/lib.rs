//! checks feature contract: watch a URL, record what it answered.
//!
//! DTOs live here, not in `checks-core`, so the wall is physical: `incidents-core`
//! reads probe history through this crate and cannot reach the store behind it.

use gabion::prelude::*;

/// One URL under watch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Check {
    pub id: String,
    pub name: String,
    pub url: String,
    /// How long a watch must sit before it is due again.
    pub interval_secs: u64,
    /// How long one attempt may take before it counts as no answer.
    pub timeout_ms: u64,
    pub created_at: u64,
}

/// What to watch. The cadence is the service's policy, not the caller's, so a
/// caller cannot schedule a probe every millisecond.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NewCheck {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckId(pub String);

/// One observation. A probe that got a `500` is still a successful probe: it
/// observed an outage. Only a request that never became a response is a miss.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Probe {
    pub check_id: String,
    pub at: u64,
    pub ok: bool,
    /// `0` when no response arrived at all.
    pub status: u16,
    pub latency_ms: u64,
    /// How many attempts the result took. More than one means the first tries
    /// failed and a backoff sat between them.
    pub attempts: u32,
    /// What went wrong, empty when nothing did.
    pub note: String,
}

/// A watch with its most recent observation, which is what a status page shows.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckReport {
    pub check: Check,
    pub last: Option<Probe>,
}

/// One error enum -> HTTP status, CLI exit code, through `ErrorKind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum CheckError {
    NotFound,
    InvalidUrl,
    /// A store or serialization failure: the request was fine, the world was
    /// not. Maps to 500 / exit 1 via `ErrorKind`, never hand-mapped.
    Internal,
    /// A failure that crossed a wire and whose detail this version does not know.
    Remote {
        kind: ErrorKind,
        message: String,
    },
}

impl CheckError {
    fn kind(&self) -> ErrorKind {
        match self {
            CheckError::NotFound => ErrorKind::NotFound,
            CheckError::InvalidUrl => ErrorKind::InvalidInput,
            CheckError::Internal => ErrorKind::Internal,
            CheckError::Remote { kind, .. } => *kind,
        }
    }
}

pub type CheckErr = ServiceError<CheckError>;

/// The public contract. **Every method in this trait is generated into every
/// surface** — CLI subcommand, HTTP route, MCP tool, typed-client method — with
/// no per-method opt-in or opt-out. The trait is the security boundary: adding a
/// method here grants it to every caller, including agents.
///
/// `ctx` is the caller (spec §10.1), built by whichever surface received the
/// request.
#[service]
pub trait CheckService {
    /// Start watching a URL. Fails with `InvalidUrl` unless it is http or https.
    async fn add(&self, ctx: &Ctx, input: NewCheck) -> Result<Check, CheckErr>;

    /// Every watch with its most recent observation.
    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<CheckReport>, CheckErr>;

    /// Probe one watch now, ignoring the schedule.
    async fn probe(&self, ctx: &Ctx, input: CheckId) -> Result<Probe, CheckErr>;

    /// Probe every watch whose interval has elapsed, and return what was probed.
    async fn sweep(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Probe>, CheckErr>;

    /// Observations for one watch, oldest first.
    async fn history(&self, ctx: &Ctx, input: CheckId) -> Result<Vec<Probe>, CheckErr>;
}

/// What a remote failure becomes in this app (see `gabion::error::FromWire`).
impl FromWire for CheckError {
    fn from_wire(wire: WireError) -> Self {
        CheckError::Remote {
            kind: wire.kind,
            message: wire.message,
        }
    }
}

/// Constructor helper: builds the wire envelope from a domain error.
pub fn check_err(detail: CheckError) -> CheckErr {
    let kind = detail.kind();
    let msg = match &detail {
        CheckError::NotFound => "check not found".to_string(),
        CheckError::InvalidUrl => "url must be http or https".to_string(),
        CheckError::Internal => "storage failed".to_string(),
        CheckError::Remote { message, .. } => message.clone(),
    };
    ServiceError::new(kind, msg, detail)
}
