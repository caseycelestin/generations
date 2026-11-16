//! The control side's state behind its one lock, and the rules that keep
//! each request's record.

use std::sync::{Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};

use super::{StatusError, TryRecvStatusError, TryWaitError};
use crate::request::{FromRealtime, Request, RequestId, ToRealtime};

pub(super) struct Shared<Q: Request> {
    pub(super) to_realtime: Producer<ToRealtime<Q>>,
    from_realtime: Consumer<FromRealtime<Q>>,
    /// One per request in flight, until its reply is taken. Reserving stops
    /// at `capacity`, so allocated to that here it never grows.
    requests: Vec<InFlight<Q>>,
    /// Permits neither opened nor dropped; each holds a slot as a request in
    /// flight does.
    pub(super) reserved: usize,
    next_id: u64,
}

impl<Q: Request> Shared<Q> {
    pub(super) fn new(
        to_realtime: Producer<ToRealtime<Q>>,
        from_realtime: Consumer<FromRealtime<Q>>,
        capacity: usize,
    ) -> Self {
        Self {
            to_realtime,
            from_realtime,
            requests: Vec::with_capacity(capacity),
            reserved: 0,
            next_id: 0,
        }
    }

    /// Slots taken, by requests in flight and by permits not yet opened.
    pub(super) fn held(&self) -> usize {
        self.requests.len() + self.reserved
    }

    pub(super) fn open(&mut self, request: Q) -> RequestId {
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
    pub(super) fn take_in(&mut self) {
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
    pub(super) fn take_reply(&mut self, id: RequestId) -> Result<Q::Reply, TryWaitError> {
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
    pub(super) fn ask(&mut self, id: RequestId) -> Result<(), StatusError> {
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
    pub(super) fn take_answer(&mut self, id: RequestId) -> Result<Q::Status, TryRecvStatusError> {
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

    /// Frees the question now if its answer has come or never will, or else
    /// when the answer arrives. Nothing to free once the record is gone.
    pub(super) fn abandon_question(&mut self, id: RequestId) {
        self.take_in();
        let Some(request) = self.requests.iter_mut().find(|request| request.id == id) else {
            return;
        };
        request.question = match request.question.take() {
            Some(Question::Asked) if request.reply.is_none() => Some(Question::Abandoned),
            _ => None,
        };
    }

    /// Frees the record now if its reply has come, or else when it does.
    pub(super) fn abandon(&mut self, id: RequestId) {
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
pub(super) fn lock<Q: Request>(shared: &Mutex<Shared<Q>>) -> MutexGuard<'_, Shared<Q>> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
