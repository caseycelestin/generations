//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use std::marker::PhantomData;

use super::{Request, RequestId};

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

#[derive(Debug)]
pub struct RealtimeEnd<Q>(PhantomData<Q>);

impl<Q: Request> RealtimeEnd<Q> {
    pub fn try_recv(&mut self) -> Option<Received<Q>> {
        todo!()
    }

    /// Sends the request's reply and ends it; its questions still out are
    /// closed. Cannot fail: the slot was reserved with the request.
    pub fn close(&mut self, permit: ReplyPermit, reply: Q::Reply) {
        todo!()
    }

    /// Leaves the request open. Cannot fail: the slot was reserved with the
    /// request.
    pub fn answer(&mut self, permit: StatusPermit, status: Q::Status) {
        todo!()
    }
}
