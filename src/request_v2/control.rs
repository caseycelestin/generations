//! The control side's end, and each request's receiver. Allocating and
//! freeing are fine here.

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use super::{ReplySender, Request};

/// The caller's end of one request's reply ring. Dropping it early is
/// safe: the real-time side's end comes back, and the ring is freed here.
pub struct ReplyReceiver<R> {
    pub(super) reply: Consumer<R>,
    /// Its one reply was taken, so an empty ring no longer means not yet.
    pub(super) taken: bool,
}

impl<R> ReplyReceiver<R> {
    /// A reply that came before its sender was dropped is still taken.
    pub fn try_wait(&mut self) -> Result<R, TryWaitError> {
        if self.taken {
            return Err(TryWaitError::Taken);
        }
        // Asked before popping: a sender dropped by now pushed any reply
        // first, so an empty ring after it means none is coming.
        let abandoned = self.reply.is_abandoned();
        let Ok(reply) = self.reply.pop() else {
            if abandoned {
                return Err(TryWaitError::Disconnected);
            }
            return Err(TryWaitError::Empty);
        };
        self.taken = true;
        Ok(reply)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryWaitError {
    #[error("no reply yet")]
    Empty,
    #[error("the reply was already taken")]
    Taken,
    /// Its sender was dropped without replying, as when the real-time side
    /// is gone.
    #[error("no reply will come")]
    Disconnected,
}

/// One request's reply ring, one deep: a request has one reply.
fn reply_ring<R>() -> (ReplySender<R>, ReplyReceiver<R>) {
    let (producer, consumer) = RingBuffer::new(1);
    let sender = ReplySender { reply: producer };
    let receiver = ReplyReceiver {
        reply: consumer,
        taken: false,
    };
    (sender, receiver)
}

pub struct ControlEnd<Q: Request> {
    /// Each request with the end its reply goes into.
    pub(super) requests: Producer<(Q, ReplySender<Q::Reply>)>,
    /// Reply ends the real-time side is done with, to drop here.
    pub(super) returned: Consumer<ReplySender<Q::Reply>>,
    /// Sent and not yet back on `returned`; `try_send` refuses at capacity.
    pub(super) in_flight: usize,
}

impl<Q: Request> ControlEnd<Q> {
    /// Hands the request back when `capacity` requests are in flight.
    pub fn try_send(&mut self, request: Q) -> Result<ReplyReceiver<Q::Reply>, Q> {
        self.take_in();
        if self.in_flight == self.requests.buffer().capacity() {
            return Err(request);
        }
        let (sender, receiver) = reply_ring();
        let pushed = self.requests.push((request, sender));
        assert!(pushed.is_ok(), "the ring holds every request in flight");
        self.in_flight += 1;
        Ok(receiver)
    }

    /// Drops every `ReplySender` the real-time side has sent back, freeing
    /// rings whose receiver is gone, and counts each out of flight.
    pub fn take_in(&mut self) {
        while let Ok(sender) = self.returned.pop() {
            drop(sender);
            self.in_flight -= 1;
        }
    }
}
