//! The control side's end, and the requests sent from it.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use super::{FromRealtime, Request, RequestId, ToRealtime};

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
        let shared = Shared {
            to_realtime,
            from_realtime,
            requests: Vec::with_capacity(capacity),
            reserved: 0,
            next_id: 0,
        };
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

    /// `Full` once `limit` slots are held, by requests in flight and by
    /// permits not yet opened.
    fn reserve(&self, limit: usize) -> Result<RequestPermit<'_, Q>, ReserveError> {
        let mut shared = lock(&self.shared);
        shared.take_in();
        if shared.to_realtime.is_abandoned() {
            return Err(ReserveError::Disconnected);
        }
        if shared.requests.len() + shared.reserved >= limit {
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
/// and its reply is dropped when it arrives.
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

struct Shared<Q: Request> {
    to_realtime: Producer<ToRealtime<Q>>,
    from_realtime: Consumer<FromRealtime<Q>>,
    /// One per request in flight, until its reply is taken. Reserving stops
    /// at `capacity`, so allocated to that here it never grows.
    requests: Vec<InFlight<Q>>,
    /// Permits neither opened nor dropped; each holds a slot as a request in
    /// flight does.
    reserved: usize,
    next_id: u64,
}

impl<Q: Request> Shared<Q> {
    fn open(&mut self, request: Q) -> RequestId {
        assert!(
            self.requests.len() < self.requests.capacity(),
            "reserving keeps the table within what it was given"
        );
        let id = RequestId(self.next_id);
        self.next_id += 1;
        self.requests.push(InFlight {
            id,
            reply: None,
            question: None,
            abandoned: false,
        });
        let pushed = self.to_realtime.push(ToRealtime::Request(id, request));
        assert!(pushed.is_ok(), "the ring holds three messages per request");
        id
    }

    /// Moves what has arrived into its request's record. For a request whose
    /// handle is gone it is dropped here instead, the reply with its record.
    fn take_in(&mut self) {
        while let Ok(message) = self.from_realtime.pop() {
            match message {
                FromRealtime::Reply(id, reply) => {
                    let index = self.index_of(id);
                    if self.requests[index].abandoned {
                        self.requests.swap_remove(index);
                    } else {
                        self.requests[index].reply = Some(reply);
                    }
                }
                FromRealtime::Status(id, status) => {
                    let index = self.index_of(id);
                    let question = &mut self.requests[index].question;
                    *question = match question.take() {
                        Some(Question::Asked) => Some(Question::Answered(status)),
                        // Its pending answer is gone, so the slot frees here.
                        Some(Question::Abandoned) => None,
                        _ => panic!("only an asked question is answered"),
                    };
                }
            }
        }
    }

    /// Takes everything in first, as this request's reply may be behind
    /// others'. Its record goes with the reply.
    fn take_reply(&mut self, id: RequestId) -> Result<Q::Reply, TryWaitError> {
        self.take_in();
        let index = self.index_of(id);
        let Some(reply) = self.requests[index].reply.take() else {
            if self.from_realtime.is_abandoned() {
                return Err(TryWaitError::Disconnected);
            }
            return Err(TryWaitError::Empty);
        };
        self.requests.swap_remove(index);
        Ok(reply)
    }

    /// `Closed` once the reply has come, so no answer would follow it.
    fn ask(&mut self, id: RequestId) -> Result<(), StatusError> {
        self.take_in();
        let index = self.index_of(id);
        let request = &mut self.requests[index];
        if request.reply.is_some() {
            return Err(StatusError::Closed);
        }
        if request.question.is_some() {
            return Err(StatusError::Busy);
        }
        if self.to_realtime.is_abandoned() {
            return Err(StatusError::Disconnected);
        }
        request.question = Some(Question::Asked);
        let pushed = self.to_realtime.push(ToRealtime::Status(id));
        assert!(pushed.is_ok(), "the ring holds three messages per request");
        Ok(())
    }

    /// `Closed` once the request has ended without answering: its reply came
    /// first, or its record has gone with the reply.
    fn take_answer(&mut self, id: RequestId) -> Result<Q::Status, TryRecvStatusError> {
        self.take_in();
        let found = self.requests.iter().position(|request| request.id == id);
        let Some(index) = found else {
            return Err(TryRecvStatusError::Closed);
        };
        let request = &mut self.requests[index];
        match request.question.take() {
            Some(Question::Answered(status)) => return Ok(status),
            unanswered => request.question = unanswered,
        }
        if request.reply.is_some() {
            return Err(TryRecvStatusError::Closed);
        }
        if self.from_realtime.is_abandoned() {
            return Err(TryRecvStatusError::Disconnected);
        }
        Err(TryRecvStatusError::Empty)
    }

    /// Frees the record now if its reply has come, or else when it does.
    fn abandon(&mut self, id: RequestId) {
        self.take_in();
        let index = self.index_of(id);
        if self.requests[index].reply.is_some() {
            self.requests.swap_remove(index);
        } else {
            self.requests[index].abandoned = true;
        }
    }

    fn index_of(&self, id: RequestId) -> usize {
        self.requests
            .iter()
            .position(|request| request.id == id)
            .expect("only a request in flight is answered")
    }
}

struct InFlight<Q: Request> {
    id: RequestId,
    /// Until taken.
    reply: Option<Q::Reply>,
    /// The one status question out, from asking until its answer is taken
    /// or, once its pending answer is gone, arrives.
    question: Option<Question<Q::Status>>,
    /// Its handle is gone, so what arrives for it is dropped.
    abandoned: bool,
}

enum Question<S> {
    Asked,
    /// Until its pending answer takes it.
    Answered(S),
    /// Its pending answer is gone; the answer is dropped on arrival.
    Abandoned,
}

/// A panic while locked leaves the rings whole, so a poisoned lock is taken
/// anyway.
fn lock<Q: Request>(shared: &Mutex<Shared<Q>>) -> MutexGuard<'_, Shared<Q>> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
