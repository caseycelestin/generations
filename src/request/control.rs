//! The control side's end, and the requests sent from it.

use std::marker::PhantomData;

use thiserror::Error;

use super::Request;

#[derive(Debug)]
pub struct ControlEnd<Q>(PhantomData<Q>);

/// Clones share one channel. Written out, as a derive would demand `Q` be
/// `Clone`.
impl<Q> Clone for ControlEnd<Q> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<Q: Request> ControlEnd<Q> {
    /// Reserves slots for a request, its cancel, one status question and the
    /// answers to both, so neither `open` nor `cancel` can fail.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        todo!()
    }
}

/// Holds the end's lock until `open`; dropped unused, it sends nothing and
/// gives its room back.
#[derive(Debug)]
pub struct RequestPermit<'a, Q>(PhantomData<&'a Q>);

impl<Q: Request> RequestPermit<'_, Q> {
    pub fn open(self, request: Q) -> RequestHandle<Q> {
        todo!()
    }
}

/// Holds its request's slot until the reply is taken or the handle is
/// dropped; an arrived reply still holds it. Dropped, the request runs on
/// and its reply is dropped when it arrives.
#[derive(Debug)]
pub struct RequestHandle<Q>(PhantomData<Q>);

impl<Q: Request> RequestHandle<Q> {
    pub fn try_wait(&mut self) -> Result<Q::Reply, TryWaitError> {
        todo!()
    }

    /// The request still replies once. Does nothing after the first call,
    /// or once the reply is taken.
    pub fn cancel(&mut self) {
        todo!()
    }

    /// Answered from the real-time side's next look at the request.
    pub fn request_status(&mut self) -> Result<PendingStatus<Q>, StatusError> {
        todo!()
    }
}

/// One status answer. Dropped early, its question still holds its slot
/// until the answer arrives.
#[derive(Debug)]
pub struct PendingStatus<Q>(PhantomData<Q>);

impl<Q: Request> PendingStatus<Q> {
    pub fn try_recv(&mut self) -> Result<Q::Status, TryRecvStatusError> {
        todo!()
    }
}

/// Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ReserveError {
    #[error("too many requests await their reply")]
    Full,
    #[error("the real-time end is gone")]
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryWaitError {
    #[error("no reply yet")]
    Empty,
    /// Its reply was taken: the request is closed.
    #[error("the reply was already taken")]
    Closed,
    /// Only once no reply is waiting: one that came first is still taken.
    #[error("the real-time end is gone; no reply will come")]
    Disconnected,
}

/// Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StatusError {
    /// One question at a time; a dropped one holds its slot until its
    /// answer arrives.
    #[error("a status question is still out")]
    Busy,
    #[error("the request is closed")]
    Closed,
    #[error("the real-time end is gone")]
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryRecvStatusError {
    #[error("no answer yet")]
    Empty,
    #[error("the answer was already taken")]
    Taken,
    /// Its request closed first: its reply has the last word.
    #[error("the request closed before answering")]
    Closed,
    #[error("the real-time end is gone; no answer will come")]
    Disconnected,
}
