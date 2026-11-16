//! The control side's end, and the requests sent from it.

use std::marker::PhantomData;

use super::{Request, ReserveError, TryRecvError};

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
    /// Reserves slots for a request, its cancel and its one reply, so
    /// neither `send` nor `cancel` can fail.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, Q>, ReserveError> {
        todo!()
    }
}

#[derive(Debug)]
pub struct RequestPermit<'a, Q>(PhantomData<&'a Q>);

impl<Q: Request> RequestPermit<'_, Q> {
    pub fn send(self, request: Q) -> RequestHandle<Q> {
        todo!()
    }
}

/// Dropping it lets the request run on; its reply is dropped when it
/// arrives.
#[derive(Debug)]
pub struct RequestHandle<Q>(PhantomData<Q>);

impl<Q: Request> RequestHandle<Q> {
    pub fn try_recv(&mut self) -> Result<Q::Reply, TryRecvError> {
        todo!()
    }

    /// The request still replies once. Only the first call sends.
    pub fn cancel(&mut self) {
        todo!()
    }
}
