//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use std::marker::PhantomData;

use super::{Full, Id, Message};

#[derive(Debug)]
pub struct RealtimeEnd<C, E>(PhantomData<(C, E)>);

impl<C, E> RealtimeEnd<C, E> {
    pub fn try_recv(&mut self) -> Option<Message<C>> {
        todo!()
    }

    /// Refused once the handle has been sent its reserved budget.
    pub fn try_send(&mut self, id: Id, event: impl Into<E>) -> Result<(), Full<E>> {
        todo!()
    }

    /// Sends the handle's last event. Its room is free again once the
    /// control side has taken it off the ring, read or not, so a handle
    /// kept unread does not hold room.
    pub fn end(&mut self, id: Id, event: impl Into<E>) -> Result<(), Full<E>> {
        todo!()
    }
}
