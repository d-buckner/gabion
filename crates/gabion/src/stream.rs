//! The streaming vocabulary: one trait, one boxed alias, two policy markers.
//!
//! This is the whole surface a contract needs to declare a stream, and it is all
//! in `gabion` so a `*-api` crate gains no new dependency to stream. The
//! transport that carries these is a separate decision (`docs/streaming.md` §5);
//! nothing here knows a socket exists.
//!
//! The one thing that cannot be decided once for all targets is `Send`. A wasm
//! host's real source is a `web_sys::WebSocket`, and `js_sys` objects are
//! `!Send` — not "hard to make `Send`", but impossible without unsafe this crate
//! forbids. So the alias is split, and `gabion::stream` is the only place in the
//! framework or in generated code allowed to write `Send` into a stream shape.
//! The split is checked by building this crate for the wasm target:
//!
//! ```text
//! cargo check -p gabion --target wasm32-unknown-unknown --all-targets
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context, Poll};

pub use futures_core::Stream;

/// A boxed stream. `Send` where a thread can hold one, and not where it cannot.
///
/// The two `cfg`s are the whole split. Anything that names a stream writes
/// `BoxStream`, never `Pin<Box<dyn Stream + Send>>`, or the wasm half silently
/// stops working and only fails on that target.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// A boxed stream. `Send` where a thread can hold one, and not where it cannot.
#[cfg(target_arch = "wasm32")]
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

/// A boxed future, split the same way [`BoxStream`] is.
///
/// A channel driven by a JS socket cannot promise more than the socket can, so
/// the frame a channel writes out is boxed under the same rule. Written here for
/// the same reason the stream alias is: one place, both `cfg`s.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// A boxed future, split the same way [`BoxStream`] is.
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

/// Every item, or nothing: the producer waits for the subscriber.
///
/// A signature marker, not a container. It appears in the return position, is
/// never serialized, never appears on the wire, and is built only by the service
/// impl. The policy it carries is not decoration: it selects the loop the
/// generated surface emits, and it is in the type so a flip moves
/// `.gabion/surfaces.lock` (`docs/streaming.md` §4.3).
pub struct Each<T: Serialize + JsonSchema> {
    inner: BoxStream<'static, T>,
}

/// The newest item is what matters: the surface's pump holds one slot per stream
/// and replaces an untaken value, so this stream cannot stall a connection.
///
/// A signature marker, exactly as [`Each`] is. The core never knows it is
/// conflating — it hands over a stream and a promise about how it is read.
pub struct Latest<T: Serialize + JsonSchema> {
    inner: BoxStream<'static, T>,
}

/// A `dyn Stream` cannot derive `Debug`, and a marker that failed `{:?}` in a
/// `Result` would break every `#[test]` that unwraps one.
macro_rules! debug_marker {
    ($name:ident) => {
        impl<T: Serialize + JsonSchema> std::fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "(<stream>)"))
            }
        }
    };
}

macro_rules! policy_marker {
    ($name:ident) => {
        impl<T: Serialize + JsonSchema> $name<T> {
            /// Wrap a source the impl was handed.
            ///
            /// A core composes streams; it does not source them
            /// (`docs/streaming.md` §8), so what arrives here came from a
            /// capability or from another service's op.
            ///
            /// Written twice because the bound differs by target: on native a
            /// thread must be able to hold the stream, and on wasm there is no
            /// thread to hold it. That is the whole `Send` split, and it stays
            /// here rather than spreading across the generators.
            #[cfg(not(target_arch = "wasm32"))]
            #[must_use]
            pub fn new<S>(stream: S) -> Self
            where
                S: Stream<Item = T> + Send + 'static,
            {
                Self {
                    inner: Box::pin(stream),
                }
            }

            /// Wrap a source the impl was handed.
            #[cfg(target_arch = "wasm32")]
            #[must_use]
            pub fn new<S>(stream: S) -> Self
            where
                S: Stream<Item = T> + 'static,
            {
                Self {
                    inner: Box::pin(stream),
                }
            }

            /// Hand the stream to something that will drive it.
            pub fn into_stream(self) -> BoxStream<'static, T> {
                self.inner
            }
        }

        impl<T: Serialize + JsonSchema> Stream for $name<T> {
            type Item = T;

            fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
                self.get_mut().inner.as_mut().poll_next(cx)
            }
        }
    };
}

policy_marker!(Each);
policy_marker!(Latest);
debug_marker!(Each);
debug_marker!(Latest);

/// Why a stream ended, as the surface reports it.
///
/// An end is always explicit. A channel that dies without one is a transport
/// failure, not a clean end (`docs/streaming.md` §7.4) — the fail-open this
/// design would otherwise ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The producer ran out of items.
    Done,
    /// Someone stopped it: a `close` frame, a cancelled `Ctx`, a dropped socket.
    Cancelled,
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a failing assertion should name the item that went missing
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::task::Waker;

    /// A source that cannot cross a thread: it holds an `Rc`, which is what a
    /// `web_sys::WebSocket` is on the wasm side.
    ///
    /// Only wasm can box it into `BoxStream`, so native builds never construct it.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[derive(Clone, Default)]
    struct RcTicker {
        next: Rc<Cell<u32>>,
        limit: u32,
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    impl RcTicker {
        fn new(limit: u32) -> Self {
            Self {
                next: Rc::new(Cell::new(1)),
                limit,
            }
        }
    }

    impl Stream for RcTicker {
        type Item = u32;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u32>> {
            let n = self.next.get();
            if n > self.limit {
                return Poll::Ready(None);
            }
            self.next.set(n + 1);
            Poll::Ready(Some(n))
        }
    }

    /// The same source, `Send`, for the target that requires it.
    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Default)]
    struct SendTicker {
        next: u32,
        limit: u32,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl Stream for SendTicker {
        type Item = u32;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u32>> {
            let this = self.get_mut();
            if this.next >= this.limit {
                return Poll::Ready(None);
            }
            this.next += 1;
            Poll::Ready(Some(this.next))
        }
    }

    /// On native the alias must keep demanding `Send`, or a host could hand a
    /// stream to a thread that cannot hold it.
    #[cfg(not(target_arch = "wasm32"))]
    const _: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<BoxStream<'static, u32>>();
    };

    /// Drives a stream to the end with no runtime: gabion is runtime-free, and a
    /// vocabulary test has no business pulling one in.
    fn collect<T>(mut stream: BoxStream<'static, T>) -> Vec<T> {
        let mut cx = Context::from_waker(Waker::noop());
        let mut out = Vec::new();
        loop {
            match stream.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(item)) => out.push(item),
                Poll::Ready(None) => return out,
                Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    /// The wasm proof, and the test that forces the `Send` split. On wasm32 the
    /// `Rc` source below is the only shape a real JS source arrives in, and
    /// boxing it is the line that does not compile if the alias carries a `Send`
    /// bound there. On native that bound is real, so the same assertion runs
    /// against a `Send` stand-in and the const above stands in for the bound.
    #[test]
    fn a_wasm_host_consumes_a_stream() {
        #[cfg(target_arch = "wasm32")]
        let stream = Latest::new(RcTicker::new(3));
        #[cfg(not(target_arch = "wasm32"))]
        let stream = Latest::new(SendTicker { next: 0, limit: 3 });

        assert_eq!(collect(stream.into_stream()), vec![1, 2, 3]);
    }

    /// A marker is a stream too: a consumer that never heard of the policy can
    /// still drain it, which is what lets one adapter serve both.
    #[test]
    fn a_policy_marker_is_a_stream() {
        #[cfg(target_arch = "wasm32")]
        let each = Each::new(RcTicker::new(4));
        #[cfg(not(target_arch = "wasm32"))]
        let each = Each::new(SendTicker { next: 0, limit: 4 });

        let mut cx = Context::from_waker(Waker::noop());
        let mut pinned = Box::pin(each);
        let mut seen = Vec::new();
        loop {
            match pinned.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(item)) => seen.push(item),
                Poll::Ready(None) => break,
                Poll::Pending => std::hint::spin_loop(),
            }
        }
        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[test]
    fn an_end_reason_is_named_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&EndReason::Done).expect("json"),
            "\"done\""
        );
        assert_eq!(
            serde_json::to_string(&EndReason::Cancelled).expect("json"),
            "\"cancelled\""
        );
        assert_eq!(
            serde_json::from_str::<EndReason>("\"cancelled\"").expect("json"),
            EndReason::Cancelled
        );
    }
}
