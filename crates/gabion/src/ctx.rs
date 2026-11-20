//! Per-call context: who is calling, for which tenant, under which trace, and
//! whether they have changed their mind.
//!
//! Spec §10.1: `Ctx` is an explicit parameter on every `#[service]` method, not a
//! task-local and not a scoped service. Two reasons, both practical:
//!
//! - it is visible in the contract, so a reviewer sees that an op takes a caller
//!   identity before it is published on four surfaces;
//! - it is trivially testable, because a test hands the impl the exact context it
//!   wants instead of arranging an environment for one.
//!
//! The adapters construct it: HTTP reads it from headers, MCP from the request's
//! `_meta`, the CLI from its own flags, and the generated client stamps the same
//! headers it reads — so an identity set at one end arrives at the other.
//!
//! Nothing here touches a runtime. Cancellation is a flag plus a waker list, not
//! a tokio primitive, because this crate compiles for wasm and for core tests.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// The header this framework uses to carry a caller identity. The generated
/// client sends it and the generated route reads it, from one constant, so the
/// two cannot drift.
pub const PRINCIPAL_HEADER: &str = "x-gabion-principal";
/// The tenant a call is scoped to.
pub const TENANT_HEADER: &str = "x-gabion-tenant";
/// The trace a call belongs to, echoed back by the generated route.
pub const TRACE_HEADER: &str = "x-gabion-trace";

/// A cancellation signal a caller can hold after the call has started.
///
/// Cloning it yields another handle to the *same* signal, which is what makes it
/// useful: the surface keeps one handle, hands the other to the caller, and a
/// host can cancel work that it can no longer reach.
#[derive(Clone, Default)]
pub struct Cancel {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
}

impl fmt::Debug for Cancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cancel")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise the flag and wake everyone waiting on [`Cancel::cancelled`]. Idempotent:
    /// a second cancel wakes nobody, because everyone already woke.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        let wakers = self.take_waiters();
        for waker in wakers {
            waker.wake();
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// A future that completes when the flag is raised, or immediately when it
    /// already is. Polling it after the flag is up must not hang: a cancelled
    /// call has to be able to check twice.
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            cancel: self.clone(),
        }
    }

    fn take_waiters(&self) -> Vec<Waker> {
        match self.inner.waiters.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }

    fn park(&self, waker: &Waker) {
        let mut v = match self.inner.waiters.lock() {
            Ok(v) => v,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !v.iter().any(|w| w.will_wake(waker)) {
            v.push(waker.clone());
        }
    }
}

/// The future returned by [`Cancel::cancelled`].
#[must_use = "a cancellation handle does nothing until it is polled"]
pub struct Cancelled {
    cancel: Cancel,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.cancel.is_cancelled() {
            return Poll::Ready(());
        }
        self.cancel.park(cx.waker());
        // Re-check after registering. Cancelling between the read and the
        // registration would otherwise lose the wake and hang the awaiter.
        if self.cancel.is_cancelled() {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

/// Who is calling, for which tenant, under which trace, with a cancellation
/// signal. Cheap to clone: the strings are small and the signal is shared.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    principal: Option<String>,
    tenant: Option<String>,
    trace: Option<String>,
    cancel: Cancel,
}

impl Ctx {
    /// A call with no identity: a timer, a test, a CLI run by the developer.
    ///
    /// It is a named constructor rather than `Default` alone so that "nobody is
    /// calling" is a decision in the code, not an omission.
    #[must_use]
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Attach the caller. `Ctx::anonymous().with_principal("agent-7")`.
    #[must_use]
    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    #[must_use]
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    #[must_use]
    pub fn with_trace(mut self, trace: impl Into<String>) -> Self {
        self.trace = Some(trace.into());
        self
    }

    /// Attach an existing cancellation signal, so a host can cancel work it can
    /// still reach.
    #[must_use]
    pub fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = cancel;
        self
    }

    #[must_use]
    pub fn principal(&self) -> Option<&str> {
        self.principal.as_deref()
    }

    #[must_use]
    pub fn tenant(&self) -> Option<&str> {
        self.tenant.as_deref()
    }

    #[must_use]
    pub fn trace(&self) -> Option<&str> {
        self.trace.as_deref()
    }

    #[must_use]
    pub fn cancel_token(&self) -> &Cancel {
        &self.cancel
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The headers that carry this context to another process. The generated
    /// client sends these and the generated route reads them, so an identity set
    /// on one side arrives on the other without a per-service mapping.
    #[must_use]
    pub fn wire_headers(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(p) = &self.principal {
            out.push((PRINCIPAL_HEADER.to_string(), p.clone()));
        }
        if let Some(t) = &self.tenant {
            out.push((TENANT_HEADER.to_string(), t.clone()));
        }
        if let Some(t) = &self.trace {
            out.push((TRACE_HEADER.to_string(), t.clone()));
        }
        out
    }

    /// Rebuild a context from the headers [`Ctx::wire_headers`] writes. Header
    /// names are compared case-insensitively because a proxy may normalise them.
    #[must_use]
    pub fn from_wire_headers<'a, I>(headers: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut ctx = Self::default();
        for (name, value) in headers {
            let name = name.to_ascii_lowercase();
            match name.as_str() {
                PRINCIPAL_HEADER => ctx.principal = Some(value.to_string()),
                TENANT_HEADER => ctx.tenant = Some(value.to_string()),
                TRACE_HEADER => ctx.trace = Some(value.to_string()),
                _ => {}
            }
        }
        ctx
    }

    /// Rebuild a context from an MCP request's `params._meta` object, which is
    /// where the MCP wire format puts caller metadata.
    #[must_use]
    pub fn from_meta(meta: &serde_json::Value) -> Self {
        let field = |key: &str| {
            meta.get(key)
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        };
        Self {
            principal: field("principal"),
            tenant: field("tenant"),
            trace: field("trace"),
            cancel: Cancel::new(),
        }
    }

    /// The `_meta` object that carries this context over MCP. The mirror of
    /// [`Ctx::from_meta`], so a client and a server agree by construction.
    #[must_use]
    pub fn to_meta(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        if let Some(p) = &self.principal {
            map.insert("principal".to_string(), p.clone().into());
        }
        if let Some(t) = &self.tenant {
            map.insert("tenant".to_string(), t.clone().into());
        }
        if let Some(t) = &self.trace {
            map.insert("trace".to_string(), t.clone().into());
        }
        serde_json::Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_the_headers() {
        let ctx = Ctx::anonymous()
            .with_principal("agent-7")
            .with_tenant("acme")
            .with_trace("t-1");
        let headers = ctx.wire_headers();
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let back = Ctx::from_wire_headers(borrowed);
        assert_eq!(back.principal(), Some("agent-7"));
        assert_eq!(back.tenant(), Some("acme"));
        assert_eq!(back.trace(), Some("t-1"));
    }

    /// A proxy is allowed to upper-case a header name; an identity that depends
    /// on the casing of a header is not a boundary.
    #[test]
    fn header_names_are_case_insensitive() {
        let back = Ctx::from_wire_headers([("X-Gabion-Principal", "root")]);
        assert_eq!(back.principal(), Some("root"));
    }

    #[test]
    fn an_anonymous_context_writes_no_headers() {
        assert!(Ctx::anonymous().wire_headers().is_empty());
        assert_eq!(Ctx::anonymous().principal(), None);
    }

    #[test]
    fn identity_survives_the_mcp_meta() {
        let ctx = Ctx::anonymous()
            .with_principal("agent-7")
            .with_tenant("acme");
        let back = Ctx::from_meta(&ctx.to_meta());
        assert_eq!(back.principal(), Some("agent-7"));
        assert_eq!(back.tenant(), Some("acme"));
        assert_eq!(back.trace(), None);
    }

    #[test]
    fn meta_that_is_not_an_object_reads_as_anonymous() {
        let back = Ctx::from_meta(&serde_json::Value::Null);
        assert_eq!(back.principal(), None);
    }
}
