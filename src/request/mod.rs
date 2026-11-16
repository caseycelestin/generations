//! Requests from control-side callers to a real-time side, each answered by
//! exactly one reply. Knows nothing of audio.
//!
//! - On the real-time side every operation is wait-free and bounded: it
//!   never allocates, frees, locks or blocks. The control side may.
//! - Memory is fixed at creation.
//! - A reservation holds room for a request, its cancel and its reply, so it
//!   is the only step that can be refused; nothing the real-time side sends
//!   is lost.
//! - Each direction is in order.
//!
//! Each side's end is in its own file, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{ControlEnd, RequestHandle, RequestPermit};
pub use realtime::{RealtimeEnd, Received, ReplyPermit};

use thiserror::Error;

/// Pairs a request type with its reply, so `send` knows what its handle
/// returns.
pub trait Request {
    type Reply;
}

/// Unique within a channel and never reused; names, not ages, so they do
/// not order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub(crate) u64);

/// Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReserveError {
    #[error("too many requests await their reply")]
    Full,
    #[error("the real-time end is gone")]
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryRecvError {
    /// Not come yet, or already taken.
    #[error("no reply to take")]
    Empty,
    /// Only once no reply is waiting: one that came first is still taken.
    #[error("the real-time end is gone; no reply will come")]
    Disconnected,
}

/// `capacity` is in requests: the most that can await their reply at once.
/// The ring out holds twice that, for each one's cancel, so neither ring
/// ever fills.
///
/// # Panics
/// If `capacity` is zero: a channel with no room refuses every request.
pub fn channel<C, R>(capacity: usize) -> (ControlEnd<C, R>, RealtimeEnd<C, R>) {
    todo!()
}
