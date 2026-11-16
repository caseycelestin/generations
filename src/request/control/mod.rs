//! The control side's end, and the requests sent from it.

mod shared;

use std::fmt;
use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use super::{FromRealtime, Request, RequestId, ToRealtime};
use shared::{Shared, lock};

/// The lock is for its clones and handles; the real-time side never takes
/// it.
pub struct ControlEnd<Q: Request> {
    shared: Arc<Mutex<Shared<Q>>>,
    /// In requests; the rings are sized from it.
    capacity: usize,
    /// The last slots of `capacity`, kept for
    /// `try_reserve_including_headroom`.
    headroom: usize,
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
        Self {
            shared: Arc::clone(&self.shared),
            capacity: self.capacity,
            headroom: self.headroom,
        }
    }
}

impl<Q: Request> ControlEnd<Q> {
    pub(super) fn new(
        to_realtime: Producer<ToRealtime<Q>>,
        from_realtime: Consumer<FromRealtime<Q>>,
        capacity: usize,
        headroom: usize,
    ) -> Self {
        let shared = Shared::new(to_realtime, from_realtime, capacity);
        Self {
            shared: Arc::new(Mutex::new(shared)),
            capacity,
            headroom,
        }
    }

    /// Reserves slots for a request, its cancel, one status question and the
    /// answers to both, so neither `open` nor `cancel` can fail. `Full` once
    /// only the headroom is free.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        self.reserve(self.capacity.saturating_sub(self.headroom))
    }

    /// As `try_reserve`, but may use the headroom too.
    pub fn try_reserve_including_headroom(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        self.reserve(self.capacity)
    }

    /// Takes in the replies that have arrived, dropping those whose handle
    /// is gone, so what they hold is freed here. Reserving does this first.
    pub fn take_in(&self) {
        lock(&self.shared).take_in();
    }

    /// `Full` once `limit` slots are held.
    fn reserve(&self, limit: usize) -> Result<RequestPermit<'_, Q>, ReserveError> {
        let mut shared = lock(&self.shared);
        shared.take_in();
        if shared.to_realtime.is_abandoned() {
            return Err(ReserveError::Disconnected);
        }
        if shared.held() >= limit {
            return Err(ReserveError::Full);
        }
        shared.reserved += 1;
        Ok(RequestPermit { end: self })
    }
}

/// Holds room for one request until `open`; dropped unused, it sends
/// nothing and gives the room back.
pub struct RequestPermit<'a, Q: Request> {
    end: &'a ControlEnd<Q>,
}

/// Written out, as a derive would demand `Q`'s reply and status be `Debug`.
impl<Q: Request> fmt::Debug for RequestPermit<'_, Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestPermit").finish_non_exhaustive()
    }
}

impl<Q: Request> RequestPermit<'_, Q> {
    pub fn open(self, request: Q) -> RequestHandle<Q> {
        // Let go before this permit's drop takes the lock to give its
        // reservation back.
        let id = lock(&self.end.shared).open(request);
        RequestHandle {
            shared: Arc::clone(&self.end.shared),
            id,
            taken: false,
            cancelled: false,
        }
    }
}

impl<Q: Request> Drop for RequestPermit<'_, Q> {
    fn drop(&mut self) {
        lock(&self.end.shared).reserved -= 1;
    }
}

/// Holds its request's slot until the reply is taken or the handle is
/// dropped; an arrived reply still holds it. Dropped, the request runs on
/// and its reply is dropped here, at once if it has come.
pub struct RequestHandle<Q: Request> {
    shared: Arc<Mutex<Shared<Q>>>,
    id: RequestId,
    /// Its reply was taken, so its record is gone.
    taken: bool,
    cancelled: bool,
}

/// Written out, as a derive would demand `Q`'s reply and status be `Debug`.
impl<Q: Request> fmt::Debug for RequestHandle<Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestHandle")
            .field("id", &self.id)
            .finish()
    }
}

impl<Q: Request> RequestHandle<Q> {
    /// Takes in every reply that has arrived, not only this one's.
    pub fn try_wait(&mut self) -> Result<Q::Reply, TryWaitError> {
        if self.taken {
            return Err(TryWaitError::Taken);
        }
        let reply = lock(&self.shared).take_reply(self.id)?;
        self.taken = true;
        Ok(reply)
    }

    /// The request still replies once. Does nothing after the first call,
    /// or once the reply is taken.
    pub fn cancel(&mut self) {
        if self.taken || self.cancelled {
            return;
        }
        self.cancelled = true;
        let message = ToRealtime::Cancel(self.id);
        let pushed = lock(&self.shared).to_realtime.push(message);
        assert!(pushed.is_ok(), "the ring holds three messages per request");
    }

    /// Answered from the real-time side's next look at the request.
    pub fn request_status(&mut self) -> Result<PendingStatus<Q>, StatusError> {
        if self.taken {
            return Err(StatusError::Closed);
        }
        lock(&self.shared).ask(self.id)?;
        Ok(PendingStatus {
            shared: Arc::clone(&self.shared),
            id: self.id,
            taken: false,
        })
    }
}

impl<Q: Request> Drop for RequestHandle<Q> {
    fn drop(&mut self) {
        if !self.taken {
            lock(&self.shared).abandon(self.id);
        }
    }
}

/// One status answer. Dropped early, its question still holds its slot
/// until the answer arrives.
pub struct PendingStatus<Q: Request> {
    shared: Arc<Mutex<Shared<Q>>>,
    /// The request asked about.
    id: RequestId,
    taken: bool,
}

/// Written out, as a derive would demand `Q`'s reply and status be `Debug`.
impl<Q: Request> fmt::Debug for PendingStatus<Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingStatus")
            .field("id", &self.id)
            .finish()
    }
}

impl<Q: Request> PendingStatus<Q> {
    pub fn try_recv(&mut self) -> Result<Q::Status, TryRecvStatusError> {
        if self.taken {
            return Err(TryRecvStatusError::Taken);
        }
        let status = lock(&self.shared).take_answer(self.id)?;
        self.taken = true;
        Ok(status)
    }
}

impl<Q: Request> Drop for PendingStatus<Q> {
    fn drop(&mut self) {
        if !self.taken {
            lock(&self.shared).abandon_question(self.id);
        }
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
    #[error("the reply was already taken")]
    Taken,
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
