//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks, except when dropped, at
//! teardown, when no deadline is left.

use rtrb::{Consumer, Producer};

/// Never dropped on the real-time side, as it may be the last end of its
/// ring: `complete` sends it back.
#[derive(Debug)]
pub struct Completion<T> {
    pub(super) value: T,
    pub(super) ring: Producer<T>,
}

#[derive(Debug)]
pub struct RealtimeEnd<T> {
    pub(super) started: Consumer<Completion<T>>,
    pub(super) returned: Producer<Producer<T>>,
}

/// Drops every value not yet received, so their transfers learn it will not
/// come back: the ring keeps them while the control end lives.
impl<T> Drop for RealtimeEnd<T> {
    fn drop(&mut self) {
        while self.started.pop().is_ok() {}
    }
}

impl<T> RealtimeEnd<T> {
    pub fn try_recv(&mut self) -> Option<Completion<T>> {
        self.started.pop().ok()
    }

    /// Takes the completion, so a transfer completes once and its value and
    /// ring always go back, to be freed on the control side. Cannot fail:
    /// each ring has room for it.
    pub fn complete(&mut self, completion: Completion<T>) {
        let Completion { value, mut ring } = completion;
        let sent = ring.push(value);
        debug_assert!(sent.is_ok(), "a return ring holds its one value");
        let returned = self.returned.push(ring);
        debug_assert!(returned.is_ok(), "the ring holds every transfer in flight");
    }
}
