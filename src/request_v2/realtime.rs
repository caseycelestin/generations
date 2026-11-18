//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks, except when dropped, at
//! teardown, when no deadline is left.

use rtrb::{Consumer, Producer};

/// Never dropped on the real-time side, as it may be the last end of its
/// ring: `reply` sends it back.
pub struct ReplySender<R> {
    pub(super) reply: Producer<R>,
}

pub struct RealtimeEnd<Q> {
    pub(super) requests: Consumer<(Q, ReplySender<Q>)>,
    pub(super) returned: Producer<ReplySender<Q>>,
}

/// Drops every request not yet read, so their receivers learn no reply is
/// coming: the ring keeps them while the control end lives.
impl<Q> Drop for RealtimeEnd<Q> {
    fn drop(&mut self) {
        while self.requests.pop().is_ok() {}
    }
}

impl<Q> RealtimeEnd<Q> {
    pub fn try_recv(&mut self) -> Option<(Q, ReplySender<Q>)> {
        self.requests.pop().ok()
    }

    /// Takes the sender and the request, so a request is answered once and
    /// both always go back, to be freed on the control side. Cannot fail:
    /// each ring has room for it.
    pub fn reply(&mut self, mut sender: ReplySender<Q>, request: Q) {
        let replied = sender.reply.push(request);
        debug_assert!(replied.is_ok(), "a reply ring holds its one reply");
        let returned = self.returned.push(sender);
        debug_assert!(returned.is_ok(), "the ring holds every request in flight");
    }
}
