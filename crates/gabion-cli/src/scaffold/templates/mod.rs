//! The files `gabion new` and `gabion feature add` write.
//!
//! Templates are text, not a build system: the scaffold copies the *walls* the
//! framework ships (the workspace metadata block, the lints, `clippy.toml`, a
//! surface lock) so an app starts correct instead of starting empty and hoping
//! someone adds them. The two that are generated rather than pasted —
//! `clippy.toml` and `[workspace.lints]` — come out of `Config::default()`, the
//! same values `gabion check` audits, so a scaffolded app cannot disagree with
//! its own gate.
//!
//! Placeholders: `%NAME%` (the app), `%FEATURE%` (the feature folder), `%SVC%`
//! (the type prefix, capitalised), `%GABION%` (path to the gabion crates).
//!
//! The hosts are in `hosts.rs`: they are composition roots, not feature shape, and
//! a host template that grows a second surface does not belong in this file.

pub(crate) const ROOT_MANIFEST: &str = r#"[workspace]
resolver = "2"
members = [
%MEMBERS%
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
publish = false

# `gabion check` reads this block. The edge table is data, so an app widens or
# tightens its own walls without patching the framework.
[workspace.metadata.gabion]
# Read by `gabion feature add` to name the crates it generates: a path like
# `features/todo/todo-api` carries the feature, not the app.
app-name = "%NAME%"
framework-crates = ["gabion", "gabion-macros", "gabion-native", "gabion-cli"]
effect-homes = []
max-file-lines = 400
max-branches = 10

[workspace.metadata.gabion.edges]
api = ["framework", "shared", "api"]
core = ["framework", "shared", "api"]
render = ["framework", "shared", "api"]
shared = ["framework"]
host = ["framework", "shared", "api", "core", "render"]
framework = ["framework"]
unclassified = []

%LINTS%

[workspace.dependencies]
gabion = { path = "%GABION%/crates/gabion" }
gabion-native = { path = "%GABION%/crates/gabion-native" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1.2"
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "time"] }
clap = "4"
"#;

pub(crate) const API_MANIFEST: &str = r#"[package]
name = "%NAME%-%FEATURE%-api"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false
description = "%FEATURE% contract: wire types + the #[service] trait. Compiles everywhere."

[features]
default = ["client", "registry"]
client = []
registry = []
server = ["dep:axum"]
cli = ["dep:clap"]
mcp = []
openapi = ["server"]
conformance = ["server", "cli", "mcp", "client"]

[dependencies]
gabion = { workspace = true }
serde = { workspace = true }
schemars = { workspace = true }
axum = { workspace = true, optional = true }
clap = { workspace = true, optional = true }

# Used only inside `#[service]` expansion, which cargo-shear cannot see. The
# tool's own escape hatch, not a deleted wall.
[package.metadata.cargo-shear]
ignored = ["serde", "axum", "clap", "schemars"]

[lints]
workspace = true
"#;

pub(crate) const API_LIB: &str = r#"//! %FEATURE% feature contract: the wire types and the trait every surface is
//! generated from.
//!
//! This crate is the wall. `%FEATURE%-core` implements the trait, `%FEATURE%-render`
//! names it, and neither can reach the other's inside.

use gabion::prelude::*;

/// One %FEATURE% record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct %SVC% {
    pub id: String,
    pub title: String,
    pub done: bool,
    pub created_at: u64,
}

/// What a caller sends to create one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct New%SVC% {
    pub title: String,
}

/// One error enum: HTTP status, MCP code and CLI exit code all come from `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum %SVC%Error {
    NotFound,
    InvalidInput,
    Internal,
    /// A failure that crossed a wire whose detail this version does not know.
    Remote { kind: ErrorKind, message: String },
}

impl %SVC%Error {
    fn kind(&self) -> ErrorKind {
        match self {
            %SVC%Error::NotFound => ErrorKind::NotFound,
            %SVC%Error::InvalidInput => ErrorKind::InvalidInput,
            %SVC%Error::Internal => ErrorKind::Internal,
            %SVC%Error::Remote { kind, .. } => *kind,
        }
    }
}

pub type %SVC%Err = ServiceError<%SVC%Error>;

/// What a remote failure becomes here. The generated client calls this, so the
/// compiler demands an answer the moment a client is built.
impl FromWire for %SVC%Error {
    fn from_wire(wire: WireError) -> Self {
        %SVC%Error::Remote {
            kind: wire.kind,
            message: wire.message,
        }
    }
}

/// Build the wire envelope from a domain error.
pub fn %FEATURE%_err(detail: %SVC%Error) -> %SVC%Err {
    let kind = detail.kind();
    let message = match &detail {
        %SVC%Error::NotFound => "not found".to_string(),
        %SVC%Error::InvalidInput => "title must not be empty".to_string(),
        %SVC%Error::Internal => "storage failed".to_string(),
        %SVC%Error::Remote { message, .. } => message.clone(),
    };
    ServiceError::new(kind, message, detail)
}

/// The public contract. **Every method in this trait is generated into every
/// surface** — CLI, HTTP, MCP, typed client — with no per-method opt-out, so the
/// trait is the security boundary. An op that must not be remote does not belong
/// here: put it in a plain trait (reachable in-proc from another feature) or in an
/// inherent `impl` on the type (reachable nowhere outside the feature).
#[service]
pub trait %SVC%Service {
    /// Add one, from `ctx`'s caller.
    async fn create(&self, ctx: &Ctx, input: New%SVC%) -> Result<%SVC%, %SVC%Err>;

    /// Everything, in creation order.
    async fn list(&self, ctx: &Ctx, _input: ()) -> Result<Vec<%SVC%>, %SVC%Err>;
}
"#;

pub(crate) const CORE_MANIFEST: &str = r#"[package]
name = "%NAME%-%FEATURE%-core"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false
description = "%FEATURE% business logic. Runtime-free: it compiles for wasm32."

[dependencies]
gabion = { workspace = true }
%NAME%-%FEATURE%-api = { path = "../%FEATURE%-api", features = ["registry"] }
serde_json = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }

[lints]
workspace = true
"#;

pub(crate) const CORE_LIB: &str = r#"//! %FEATURE% business logic. Runtime-free and wasm-safe: every effect arrives
//! through `Caps`, so this file never touches a clock, a file, a socket or a
//! thread — and `cargo build -p %NAME%-%FEATURE%-core --target wasm32-unknown-unknown`
//! is the proof.

use gabion::prelude::*;
use %NAME%_%FEATURE%_api::{
    %FEATURE%_err, %SVC%, %SVC%Err, %SVC%Error, %SVC%Service, New%SVC%,
};

pub struct %SVC%ServiceImpl<C: Caps> {
    caps: C,
}

impl<C: Caps> %SVC%ServiceImpl<C> {
    #[must_use]
    pub fn new(caps: C) -> Self {
        Self { caps }
    }

    fn key(id: &str) -> String {
        format!("%FEATURE%/{id}")
    }

    async fn encode(todo: &%SVC%) -> Result<String, %SVC%Err> {
        serde_json::to_string(todo).map_err(|_| %FEATURE%_err(%SVC%Error::Internal))
    }
}

#[async_trait]
impl<C: Caps> %SVC%Service for %SVC%ServiceImpl<C> {
    async fn create(&self, _ctx: &Ctx, input: New%SVC%) -> Result<%SVC%, %SVC%Err> {
        if input.title.trim().is_empty() {
            return Err(%FEATURE%_err(%SVC%Error::InvalidInput));
        }
        let now = self.caps.clock().now().await;
        let nonce = self.caps.random().next_u64().await as u32;
        let todo = %SVC% {
            id: format!("t{}-{nonce:08x}", now.0),
            title: input.title,
            done: false,
            created_at: now.0,
        };
        let body = Self::encode(&todo).await?;
        self.caps
            .store()
            .set(&Self::key(&todo.id), &body)
            .await
            .map_err(|_| %FEATURE%_err(%SVC%Error::Internal))?;
        Ok(todo)
    }

    async fn list(&self, _ctx: &Ctx, _input: ()) -> Result<Vec<%SVC%>, %SVC%Err> {
        let rows = self
            .caps
            .store()
            .list("%FEATURE%/")
            .await
            .map_err(|_| %FEATURE%_err(%SVC%Error::Internal))?;
        rows.iter()
            .map(|(_, body)| {
                serde_json::from_str::<%SVC%>(body)
                    .map_err(|_| %FEATURE%_err(%SVC%Error::Internal))
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a failing assertion should name the call
mod tests {
    use super::*;
    use gabion::testing::TestCaps;

    #[tokio::test]
    async fn create_then_list() {
        let svc = %SVC%ServiceImpl::new(TestCaps::new());
        let ctx = Ctx::anonymous();
        svc.create(&ctx, New%SVC% { title: "milk".into() })
            .await
            .unwrap();
        assert_eq!(svc.list(&ctx, ()).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_blank_title_is_invalid_input() {
        let svc = %SVC%ServiceImpl::new(TestCaps::new());
        let ctx = Ctx::anonymous();
        let err = svc
            .create(&ctx, New%SVC% { title: " ".into() })
            .await
            .unwrap_err();
        assert_eq!(err.kind.http_status(), 400);
    }
}
"#;

pub(crate) const RENDER_MANIFEST: &str = r#"[package]
name = "%NAME%-%FEATURE%-render"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false
description = "%FEATURE% UI. Depends on the contract only — never on %FEATURE%-core."

[dependencies]
gabion = { workspace = true }
%NAME%-%FEATURE%-api = { path = "../%FEATURE%-api" }

[lints]
workspace = true
"#;

pub(crate) const RENDER_LIB: &str = r#"//! %FEATURE% UI. It can name the service trait and the wire types, and nothing
//! from `%FEATURE%-core`: the host injects the impl, so this crate compiles
//! identically for a CLI, a desktop window and a browser page.
//!
//! The renderer is pluggable on purpose — this one prints. A Dioxus screen would
//! live in the same file and call the same methods on [`Svc::inner`].

use gabion::prelude::Ctx;
use %NAME%_%FEATURE%_api::{%SVC%, %SVC%Err, %SVC%Service, New%SVC%};
use std::sync::Arc;

/// The service handle a host injects, plus the caller it acts as.
#[derive(Clone)]
pub struct Svc {
    pub inner: Arc<dyn %SVC%Service + Send + Sync>,
    pub ctx: Ctx,
}

impl Svc {
    #[must_use]
    pub fn new(inner: Arc<dyn %SVC%Service + Send + Sync>) -> Self {
        Self {
            inner,
            ctx: Ctx::anonymous(),
        }
    }

    #[must_use]
    pub fn with_ctx(inner: Arc<dyn %SVC%Service + Send + Sync>, ctx: Ctx) -> Self {
        Self { inner, ctx }
    }
}

/// Draw the list: the whole of the "view" layer for this feature.
pub async fn render_list(svc: &Svc) -> Result<String, %SVC%Err> {
    let items = svc.inner.list(&svc.ctx, ()).await?;
    if items.is_empty() {
        return Ok("nothing yet\n".to_string());
    }
    Ok(items
        .iter()
        .map(|item| {
            let mark = if item.done { "x" } else { " " };
            format!("[{mark}] {}\n", item.title)
        })
        .collect())
}

/// A user action: the "controller" layer, which is one call on the contract.
///
/// `pub(crate)` until something outside this crate composes it: in a render crate
/// `pub` means "another feature may use this", and `over-broad-visibility` fails
/// the gate for a public item nothing names.
pub(crate) async fn on_add(svc: &Svc, title: &str) -> Result<%SVC%, %SVC%Err> {
    svc.inner
        .create(
            &svc.ctx,
            New%SVC% {
                title: title.to_string(),
            },
        )
        .await
}
"#;

pub(crate) mod docs;
pub(crate) mod hosts;

pub(crate) use docs::README;
