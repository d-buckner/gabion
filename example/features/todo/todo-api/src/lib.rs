//! todo feature contract. The renderer may import THIS crate only —
//! never todo-core. DTOs live here, not in core, so the wall is physical.

use sietch::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTodo {
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub created_at: u64,
}

/// One error enum -> HTTP status, MCP error code, CLI exit code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TodoError {
    NotFound,
    EmptyTitle,
}

impl TodoError {
    fn kind(&self) -> ErrorKind {
        match self {
            TodoError::NotFound => ErrorKind::NotFound,
            TodoError::EmptyTitle => ErrorKind::InvalidInput,
        }
    }
}

pub type TodoErr = ServiceError<TodoError>;

#[service]
pub trait TodoService {
    #[surface(cli, http)]
    async fn create(&self, input: CreateTodo) -> Result<Todo, TodoErr>;

    #[surface(cli, http)]
    async fn complete(&self, input: TodoId) -> Result<Todo, TodoErr>;

    #[surface(cli, http)]
    async fn list(&self, _input: ()) -> Result<Vec<Todo>, TodoErr>;

    /// Explicit opt-out: internal ops get no surface, and the
    /// surface-completeness rule is satisfied by the attribute's presence.
    #[internal]
    async fn recompute_stats(&self, _input: ()) -> Result<(), TodoErr>;
}

/// Constructor helper: builds the wire envelope from a domain error.
pub fn todo_err(detail: TodoError) -> TodoErr {
    let kind = detail.kind();
    let msg = match &detail {
        TodoError::NotFound => "todo not found".to_string(),
        TodoError::EmptyTitle => "title must not be empty".to_string(),
    };
    ServiceError::new(kind, msg, detail)
}
