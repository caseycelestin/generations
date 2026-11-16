//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use std::fmt;

use rtrb::{Consumer, Producer};

use super::{FromRealtime, Request, RequestId, ToRealtime};

/// Consumed by `close`, so a request is answered at most once. Dropped
/// unanswered, it holds its request's slots for good: only right when the
/// channel is going away.
#[derive(Debug)]
pub struct ReplyPermit {
    id: RequestId,
}

impl ReplyPermit {
    /// To match its cancel and status questions.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

#[cfg(test)]
impl ReplyPermit {
    /// For tests of what holds a permit, without a channel to receive one.
    pub(crate) fn for_test(id: u64) -> Self {
        Self { id: RequestId(id) }
    }
}

/// For one status question. Dropped unanswered, it holds its slot for good,
/// unless its request is answered by now: the control side then has its
/// reply.
#[derive(Debug)]
pub struct StatusPermit {
    id: RequestId,
}

impl StatusPermit {
    /// The request asked about.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

#[derive(Debug)]
pub enum Received<Q> {
    Request(ReplyPermit, Q),
    /// Names a request already received, which may be answered by now.
    Cancel(RequestId),
    /// Asks the status of a request already received, which may be answered
    /// by now.
    Status(StatusPermit),
}

pub struct RealtimeEnd<Q: Request> {
    from_control: Consumer<ToRealtime<Q>>,
    to_control: Producer<FromRealtime<Q>>,
}

/// Written out, as a derive would demand `Q`'s reply and status be `Debug`.
impl<Q: Request> fmt::Debug for RealtimeEnd<Q> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealtimeEnd").finish_non_exhaustive()
    }
}

impl<Q: Request> RealtimeEnd<Q> {
    pub(super) fn new(
        from_control: Consumer<ToRealtime<Q>>,
        to_control: Producer<FromRealtime<Q>>,
    ) -> Self {
        Self {
            from_control,
            to_control,
        }
    }

    pub fn try_recv(&mut self) -> Option<Received<Q>> {
        let received = match self.from_control.pop().ok()? {
            ToRealtime::Request(id, request) => Received::Request(ReplyPermit { id }, request),
            ToRealtime::Cancel(id) => Received::Cancel(id),
            ToRealtime::Status(id) => Received::Status(StatusPermit { id }),
        };
        Some(received)
    }

    /// Sends the request's reply and ends it; its questions still out are
    /// closed. Cannot fail: the slot was reserved with the request.
    pub fn close(&mut self, permit: ReplyPermit, reply: Q::Reply) {
        let pushed = self.to_control.push(FromRealtime::Reply(permit.id, reply));
        debug_assert!(pushed.is_ok(), "the ring holds two messages per request");
    }

    /// Leaves the request open. Cannot fail: the slot was reserved with the
    /// request.
    pub fn answer(&mut self, permit: StatusPermit, status: Q::Status) {
        todo!()
    }
}
