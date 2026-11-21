//! One opened channel, and the codec that turns its frames into a typed stream.
//!
//! The channel is boxed behind a driver so a native host can sit a
//! `tokio_tungstenite` socket behind it and a wasm host a `web_sys::WebSocket`,
//! with the same shape in front. The codec is here rather than in a host because
//! it carries the two rules that must not be reimplemented per transport: an end
//! that is not stated is not a clean end (§7.4), and a frame from another
//! namespace is refused, not ignored (§5).

use crate::frames::{Frame, StreamError};
use crate::stream::{BoxFuture, BoxStream};

pub use codec::Items;

mod codec;

/// The send half of a channel, boxed the way [`crate::caps::Timer`] boxes a wait,
/// so a surface's pump can hold one.
pub type SendFrame<'a> = BoxFuture<'a, Result<(), StreamError>>;

/// What a host provides behind a [`Channel`].
///
/// Implementations decide what a second [`Channel::incoming`] means. The codec
/// calls it once.
pub trait ChannelDriver {
    /// Write one frame.
    fn send(&self, frame: Frame) -> SendFrame<'_>;

    /// Take frames in, until the channel is finished or dead.
    fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>>;

    /// Stop the channel. Idempotent, and never blocks.
    fn close(&self);
}

/// A boxed driver. `Send + Sync` where a thread can hold one, and not where it
/// cannot — the same split as [`BoxStream`], for the same reason.
#[cfg(not(target_arch = "wasm32"))]
type Driver = std::sync::Arc<dyn ChannelDriver + Send + Sync>;

/// A boxed driver.
#[cfg(target_arch = "wasm32")]
type Driver = std::sync::Arc<dyn ChannelDriver>;

/// One opened channel: send frames out, take frames in, close it.
///
/// A handle, not the socket: cloning one is cloning a reference to the same
/// channel, which is what a client needs — the connection is shared by every
/// stream and every `call` of the feature that opened it. Dropping the last handle
/// is what closes it.
#[derive(Clone)]
pub struct Channel {
    driver: Driver,
}

impl Channel {
    /// Wrap a host's driver.
    ///
    /// Written twice for the same reason [`BoxStream`] is split: on native a thread
    /// must be able to hold the channel, and on wasm there is no thread to hold it.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn new(driver: impl ChannelDriver + Send + Sync + 'static) -> Self {
        Self {
            driver: std::sync::Arc::new(driver),
        }
    }

    /// Wrap a host's driver.
    #[cfg(target_arch = "wasm32")]
    #[must_use]
    pub fn new(driver: impl ChannelDriver + 'static) -> Self {
        Self {
            driver: std::sync::Arc::new(driver),
        }
    }

    /// Write one frame.
    pub fn send(&self, frame: Frame) -> SendFrame<'_> {
        self.driver.send(frame)
    }

    /// Take frames in.
    pub fn incoming(&self) -> BoxStream<'static, Result<Frame, StreamError>> {
        self.driver.incoming()
    }

    /// Stop the channel.
    pub fn close(&self) {
        self.driver.close();
    }
}

/// A driver is a socket, and a socket has nothing useful to print.
impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Channel(<driver>)")
    }
}
