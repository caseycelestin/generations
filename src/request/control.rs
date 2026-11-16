//! The control side's end, and the requests sent from it.

use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use super::{FromRealtime, Request, RequestId, ToRealtime};

/// The lock is for its clones and handles; the real-time side never takes
/// it.
pub struct ControlEnd<Q: Request> {
    shared: Arc<Mutex<Shared<Q>>>,
}

/// Written out, as a derive would demand `Q`'s reply and status be `Debug`.
impl<Q: Request> fmt::Debug for ControlEnd<Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlEnd").finish_non_exhaustive()
    }
}

/// Clones share one channel. Written out, as a derive would demand `Q` be
/// `Clone`.
impl<Q: Request> Clone for ControlEnd<Q> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<Q: Request> ControlEnd<Q> {
    pub(super) fn new(
        to_realtime: Producer<ToRealtime<Q>>,
        from_realtime: Consumer<FromRealtime<Q>>,
        capacity: usize,
        headroom: usize,
    ) -> Self {
        let shared = Shared {
            to_realtime,
            from_realtime,
            capacity,
            headroom,
            requests: Vec::with_capacity(capacity),
        };
        Self {
            shared: Arc::new(Mutex::new(shared)),
        }
    }

    /// Reserves slots for a request, its cancel, one status question and the
    /// answers to both, so neither `open` nor `cancel` can fail. `Full` once
    /// only the headroom is free.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        todo!()
    }

    /// As `try_reserve`, but may use the headroom too.
    pub fn try_reserve_including_headroom(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        todo!()
    }

    /// Takes in the replies that have arrived, dropping those whose handle
    /// is gone, so what they hold is freed here. Reserving does this first.
    pub fn take_in(&self) {
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

struct Shared<Q: Request> {
    to_realtime: Producer<ToRealtime<Q>>,
    from_realtime: Consumer<FromRealtime<Q>>,
    /// In requests; the rings are sized from it.
    capacity: usize,
    /// The last slots of `capacity`, kept for
    /// `try_reserve_including_headroom`.
    headroom: usize,
    /// One per request in flight, until its reply is taken. Reserving stops
    /// at `capacity`, so allocated to that here it never grows.
    requests: Vec<InFlight>,
}

struct InFlight {
    id: RequestId,
}

/// A panic while locked leaves the rings whole, so a poisoned lock is taken
/// anyway.
fn lock<Q: Request>(shared: &Mutex<Shared<Q>>) -> MutexGuard<'_, Shared<Q>> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
