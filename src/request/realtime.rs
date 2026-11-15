//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use std::marker::PhantomData;

use super::RequestId;

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
