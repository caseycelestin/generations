//! The control side's end, and the handles opened from it.

use std::marker::PhantomData;

use super::{Full, ReserveError};

#[derive(Debug)]
pub struct ControlEnd<C, E>(PhantomData<(C, E)>);

/// Clones share one channel. Written out, as a derive would demand `C` and
/// `E` be `Clone`.
impl<C, E> Clone for ControlEnd<C, E> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<C, E> ControlEnd<C, E> {
    /// For a handle that is sent one event, its end.
    pub fn try_reserve(&self) -> Result<Permit<'_, C, E>, ReserveError> {
        todo!()
    }

    /// For a handle that is sent `events` in its life, its end included.
    pub fn try_reserve_many(&self, events: usize) -> Result<Permit<'_, C, E>, ReserveError> {
        todo!()
    }

    /// Sent under the control end's own id, for commands about no single
    /// handle.
    pub fn try_send(&self, command: impl Into<C>) -> Result<(), Full<C>> {
        todo!()
    }
}

/// Room for one handle's opening command and its events, so `open` cannot
/// fail.
#[derive(Debug)]
pub struct Permit<'a, C, E>(PhantomData<&'a (C, E)>);

impl<C, E> Permit<'_, C, E> {
    pub fn open(self, command: impl Into<C>) -> Handle<C, E> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Handle<C, E>(PhantomData<(C, E)>);

impl<C, E> Handle<C, E> {
    pub fn try_send(&self, command: impl Into<C>) -> Result<(), Full<C>> {
        todo!()
    }

    /// Events wait per handle until read; what to keep of them is the
    /// caller's.
    pub fn try_recv(&mut self) -> Option<E> {
        todo!()
    }
}
