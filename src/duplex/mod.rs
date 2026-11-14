//! A two-way channel between one real-time side and any number of
//! control-side callers, about things that live on the real-time side:
//! commands go to a thing, events come from one. Knows nothing of audio.
//!
//! - On the real-time side every operation is wait-free and bounded: it
//!   never allocates, frees, locks or blocks. The control side may.
//! - Memory is fixed at creation.
//! - A control-side send can be refused, and hands its value back. Nothing
//!   the real-time side sends is lost.
//! - The real-time side never drops what it receives; what it discards goes
//!   back or to the collector.
//! - Each direction is in order.
//!
//! Each side's end is in its own file, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{ControlEnd, Handle, Permit};
pub use realtime::RealtimeEnd;

use thiserror::Error;

/// Names a handle: commands carry their sender, events their recipient.
/// Unique within one channel and never reused; ids are names, not ages, so
/// they do not order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub(crate) u64);

#[derive(Debug)]
pub struct Message<T> {
    pub id: Id,
    pub data: T,
}

/// A refused send, holding what was sent.
#[derive(Debug, Error)]
#[error("no room for it on the ring")]
pub struct Full<T>(pub T);

/// Why a reservation was refused; nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReserveError {
    /// The real-time side has not caught up with the commands already sent.
    #[error("the command ring is full")]
    CommandsFull,
    /// Open handles hold every event slot.
    #[error("open handles hold every event slot")]
    EventsFull,
}

#[derive(Debug, Clone, Copy)]
pub struct Capacity {
    /// How many commands can wait for the real-time side.
    pub commands: usize,
    /// Event slots, shared out by reservation; each open handle holds its
    /// budget, so this also caps how many are open.
    pub events: usize,
}

pub fn channel<C, E>(capacity: Capacity) -> (ControlEnd<C, E>, RealtimeEnd<C, E>) {
    todo!()
}
