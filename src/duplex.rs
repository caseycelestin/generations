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

use std::marker::PhantomData;

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

#[derive(Debug)]
pub struct ControlEnd<C, E>(PhantomData<(C, E)>);

/// Clones share one channel. Written out, as a derive would demand `C` and
/// `E` be `Clone`.
impl<C, E> Clone for ControlEnd<C, E> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<C, E> ControlEnd<C, E> {
    /// For a handle that is sent one event, its end.
    pub fn try_reserve(&self) -> Result<Permit<'_, C, E>, ReserveError> {
        todo!()
    }

    /// For a handle that is sent `events` in its life, its end included.
    pub fn try_reserve_many(&self, events: usize) -> Result<Permit<'_, C, E>, ReserveError> {
        todo!()
    }

    /// Sent under the control end's own id, for commands about no single
    /// handle.
    pub fn send(&self, command: impl Into<C>) -> Result<(), Full<C>> {
        todo!()
    }
}

/// Room for one handle's opening command and its events, so `open` cannot
/// fail.
#[derive(Debug)]
pub struct Permit<'a, C, E>(PhantomData<&'a (C, E)>);

impl<C, E> Permit<'_, C, E> {
    pub fn open(self, command: impl Into<C>) -> Handle<C, E> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Handle<C, E>(PhantomData<(C, E)>);

impl<C, E> Handle<C, E> {
    pub fn send(&self, command: impl Into<C>) -> Result<(), Full<C>> {
        todo!()
    }

    /// Events wait per handle until read; what to keep of them is the
    /// caller's.
    pub fn try_recv(&mut self) -> Option<E> {
        todo!()
    }
}

#[derive(Debug)]
pub struct RealtimeEnd<C, E>(PhantomData<(C, E)>);

impl<C, E> RealtimeEnd<C, E> {
    pub fn try_recv(&mut self) -> Option<Message<C>> {
        todo!()
    }

    /// Refused once the handle has been sent its reserved budget.
    pub fn send(&mut self, id: Id, event: impl Into<E>) -> Result<(), Full<E>> {
        todo!()
    }

    /// Sends the handle's last event. Its room is free again once the
    /// control side has taken it off the ring, read or not, so a handle
    /// kept unread does not hold room.
    pub fn end(&mut self, id: Id, event: impl Into<E>) -> Result<(), Full<E>> {
        todo!()
    }
}
