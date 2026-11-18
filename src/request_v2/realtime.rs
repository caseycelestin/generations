//! The real-time side's end. Everything here is wait-free and bounded: it
//! never allocates, frees, locks or blocks.

use rtrb::{Consumer, Producer};

use super::Request;

/// Held by the real-time side while it runs one request. Goes back on
/// `returned` once used, never dropped there: it may be the last end of its
/// ring.
pub struct ReplySender<R> {
    pub(super) reply: Producer<R>,
}

pub struct RealtimeEnd<Q: Request> {
    pub(super) requests: Consumer<(Q, ReplySender<Q::Reply>)>,
    pub(super) returned: Producer<ReplySender<Q::Reply>>,
}

/// Drops every request not yet read, so their receivers learn no reply is
/// coming: the ring keeps them while the control end lives.
impl<Q: Request> Drop for RealtimeEnd<Q> {
    fn drop(&mut self) {
        while self.requests.pop().is_ok() {}
    }
}

impl<Q: Request> RealtimeEnd<Q> {
    pub fn try_recv(&mut self) -> Option<(Q, ReplySender<Q::Reply>)> {
        self.requests.pop().ok()
    }

    /// Takes the sender, so a request is answered once and its sender
    /// always goes back. Cannot fail: each ring has room for it.
    pub fn reply(&mut self, mut sender: ReplySender<Q::Reply>, reply: Q::Reply) {
        let replied = sender.reply.push(reply);
        debug_assert!(replied.is_ok(), "a reply ring holds its one reply");
        let returned = self.returned.push(sender);
        debug_assert!(returned.is_ok(), "the ring holds every request in flight");
    }
}
