//! The control side's end, and the requests sent from it.

use std::marker::PhantomData;

use super::{RequestId, ReserveError, TryRecvError};

#[derive(Debug)]
pub struct ControlEnd<C, R>(PhantomData<(C, R)>);

/// Clones share one channel. Written out, as a derive would demand `C` and
/// `R` be `Clone`.
impl<C, R> Clone for ControlEnd<C, R> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<C, R> ControlEnd<C, R> {
    /// Reserves a command slot for a request and a slot for its one reply,
    /// so the permit's `send` cannot fail.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, C, R>, ReserveError> {
        todo!()
    }
}

#[derive(Debug)]
pub struct RequestPermit<'a, C, R>(PhantomData<&'a (C, R)>);

impl<C, R> RequestPermit<'_, C, R> {
    pub fn send(self, request: impl Into<C>) -> RequestHandle<C, R> {
        todo!()
    }
}

/// Dropping it lets the request run on; its reply is dropped when it
/// arrives.
#[derive(Debug)]
pub struct RequestHandle<C, R>(PhantomData<(C, R)>);

impl<C, R> RequestHandle<C, R> {
    /// For a later request to name this one, which may be answered by the
    /// time that arrives.
    pub fn id(&self) -> RequestId {
        todo!()
    }

    pub fn try_recv(&mut self) -> Result<R, TryRecvError> {
        todo!()
    }
}
