//! Requests from control-side callers to a real-time side, each answered by
//! exactly one reply. Knows nothing of audio.
//!
//! - On the real-time side every operation is wait-free and bounded: it
//!   never allocates, frees, locks or blocks. The control side may.
//! - Memory is fixed at creation.
//! - A reservation holds room for a request and its reply, so it is the only
//!   step that can be refused; nothing the real-time side sends is lost.
//! - Each direction is in order.

use std::marker::PhantomData;

use thiserror::Error;

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
/// Both rings hold that many, so neither ever fills.
///
/// # Panics
/// If `capacity` is zero: a channel with no room refuses every request.
pub fn channel<C, R>(capacity: usize) -> (ControlEnd<C, R>, RealtimeEnd<C, R>) {
    todo!()
}

#[derive(Debug)]
pub struct ControlEnd<C, R>(PhantomData<(C, R)>);

/// Clones share one channel. Written out, as a derive would demand `C` and
/// `R` be `Clone`.
impl<C, R> Clone for ControlEnd<C, R> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<C, R> ControlEnd<C, R> {
    /// Reserves a command slot for a request and a slot for its one reply,
    /// so the permit's `send` cannot fail.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, C, R>, ReserveError> {
        todo!()
    }
}

#[derive(Debug)]
pub struct RequestPermit<'a, C, R>(PhantomData<&'a (C, R)>);

impl<C, R> RequestPermit<'_, C, R> {
    pub fn send(self, request: impl Into<C>) -> RequestHandle<C, R> {
        todo!()
    }
}

/// Dropping it lets the request run on; its reply is dropped when it
/// arrives.
#[derive(Debug)]
pub struct RequestHandle<C, R>(PhantomData<(C, R)>);

impl<C, R> RequestHandle<C, R> {
    /// For a later request to name this one, which may be answered by the
    /// time that arrives.
    pub fn id(&self) -> RequestId {
        todo!()
    }

    pub fn try_recv(&mut self) -> Result<R, TryRecvError> {
        todo!()
    }
}

/// Consumed by `reply`, so a request is answered at most once. Dropped
/// unanswered, it holds its request's slots for good: only right when the
/// channel is going away.
#[derive(Debug)]
pub struct ReplyPermit {
    id: RequestId,
}

impl ReplyPermit {
    /// To match a later request that names this one.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

#[derive(Debug)]
pub struct RealtimeEnd<C, R>(PhantomData<(C, R)>);

impl<C, R> RealtimeEnd<C, R> {
    pub fn try_recv(&mut self) -> Option<(ReplyPermit, C)> {
        todo!()
    }

    /// Cannot fail: the slot was reserved with the request.
    pub fn reply(&mut self, permit: ReplyPermit, reply: impl Into<R>) {
        todo!()
    }
}
