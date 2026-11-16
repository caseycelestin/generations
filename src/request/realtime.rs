//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use std::marker::PhantomData;

use super::{Request, RequestId};

/// Consumed by `reply`, so a request is answered at most once. Dropped
/// unanswered, it holds its request's slots for good: only right when the
/// channel is going away.
#[derive(Debug)]
pub struct ReplyPermit {
    id: RequestId,
}

impl ReplyPermit {
    /// To match its cancel.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

#[derive(Debug)]
pub enum Received<Q> {
    Request(ReplyPermit, Q),
    /// Names a request already received, which may be answered by now.
    Cancel(RequestId),
}

#[derive(Debug)]
pub struct RealtimeEnd<Q>(PhantomData<Q>);

impl<Q: Request> RealtimeEnd<Q> {
    pub fn try_recv(&mut self) -> Option<Received<Q>> {
        todo!()
    }

    /// Cannot fail: the slot was reserved with the request.
    pub fn reply(&mut self, permit: ReplyPermit, reply: Q::Reply) {
        todo!()
    }
}
