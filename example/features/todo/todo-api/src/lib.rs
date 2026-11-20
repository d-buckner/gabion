//! todo feature contract. The renderer may import THIS crate only —
//! never todo-core. DTOs live here, not in core, so the wall is physical.

use gabion::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTodo {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TodoId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub created_at: u64,
}

/// One error enum -> HTTP status, MCP error code, CLI exit code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum TodoError {
    NotFound,
    EmptyTitle,
    /// A store or serialization failure: the request was fine, the world was
    /// not. Maps to 500 / -32603 / exit 1 via ErrorKind, never hand-mapped.
    Internal,
    /// A failure that crossed a wire and whose detail this version does not
    /// know — an older or newer server, or a transport failure that never had a
    /// detail at all. The kind and message always survive; the variant cannot.
    Remote {
        kind: ErrorKind,
        message: String,
    },
}

impl TodoError {
    fn kind(&self) -> ErrorKind {
        match self {
            TodoError::NotFound => ErrorKind::NotFound,
            TodoError::EmptyTitle => ErrorKind::InvalidInput,
            TodoError::Internal => ErrorKind::Internal,
            TodoError::Remote { kind, .. } => *kind,
        }
    }
}

pub type TodoErr = ServiceError<TodoError>;

/// The public contract. **Every method in this trait is generated into every
/// surface** — CLI subcommand, HTTP route, MCP tool, typed-client method — with
/// no per-method opt-in or opt-out. That makes the trait itself the security
/// boundary: adding a method here grants it to every caller, including agents.
/// An op that must not be remote does not belong in this trait.
///
/// `ctx` is the caller (spec §10.1): the generated surfaces build it from the
/// request they received, so an op can tell who called it without a task-local
/// and without trusting a global.
#[service]
pub trait TodoService {
    /// Add one todo. Fails with `EmptyTitle` when the title is blank.
    async fn create(&self, ctx: &Ctx, input: CreateTodo) -> Result<Todo, TodoErr>;

    /// Mark one todo done. Fails with `NotFound` when the id is unknown.
    async fn complete(&self, ctx: &Ctx, input: TodoId) -> Result<Todo, TodoErr>;

    /// Every todo, in the order they were created.
    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<Todo>, TodoErr>;
}

/// What a remote failure becomes in this app. The client generates calls to
/// `TodoErr::from_wire`, so the compiler demands this impl the moment a client is
/// built: an app cannot ship a typed client that has no answer for a failure it
/// does not recognise.
impl FromWire for TodoError {
    fn from_wire(wire: WireError) -> Self {
        TodoError::Remote {
            kind: wire.kind,
            message: wire.message,
        }
    }
}

/// The middle rung of the ladder: reachable in-proc from another feature
/// through `todo-api`, but no surface is generated for it, so no CLI flag, HTTP
/// route, or agent tool can reach it. Purely-local internals skip this rung and
/// live as inherent methods on the impl in `todo-core` instead.
#[async_trait]
pub trait TodoInternals {
    async fn recompute_stats(&self) -> Result<(), TodoErr>;
}

/// Constructor helper: builds the wire envelope from a domain error.
pub fn todo_err(detail: TodoError) -> TodoErr {
    let kind = detail.kind();
    let msg = match &detail {
        TodoError::NotFound => "todo not found".to_string(),
        TodoError::EmptyTitle => "title must not be empty".to_string(),
        TodoError::Internal => "storage failed".to_string(),
        TodoError::Remote { message, .. } => message.clone(),
    };
    ServiceError::new(kind, msg, detail)
}
