//! The control side's end, and the requests sent from it.

use std::marker::PhantomData;

use super::{Request, ReserveError, TryRecvError};

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
    /// Reserves slots for a request, its cancel and its one reply, so
    /// neither `send` nor `cancel` can fail.
    pub fn try_reserve(&self) -> Result<RequestPermit<'_, C, R>, ReserveError> {
        todo!()
    }
}

#[derive(Debug)]
pub struct RequestPermit<'a, C, R>(PhantomData<&'a (C, R)>);

impl<C, R> RequestPermit<'_, C, R> {
    pub fn send<Q>(self, request: Q) -> RequestHandle<Q::Reply, C, R>
    where
        Q: Request + Into<C>,
        Q::Reply: TryFrom<R>,
    {
        todo!()
    }
}

/// Dropping it lets the request run on; its reply is dropped when it
/// arrives.
#[derive(Debug)]
pub struct RequestHandle<T, C, R>(PhantomData<(T, C, R)>);

impl<T, C, R> RequestHandle<T, C, R> {
    /// # Panics
    /// If the reply is not a `T`: the real-time side broke its contract.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError>
    where
        T: TryFrom<R>,
    {
        todo!()
    }

    /// The request still replies once. Only the first call sends.
    pub fn cancel(&mut self) {
        todo!()
    }
}
