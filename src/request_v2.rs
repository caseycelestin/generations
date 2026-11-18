//! Requests from control-side callers to a real-time side that only open
//! and return: each is answered by exactly one reply, and nothing else
//! crosses. Each request brings its own reply ring, made on the control
//! side; the real-time side sends that ring's end back once it has
//! replied, so it is freed on the control side. Knows nothing of audio.

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

/// Pairs a request type with its reply.
pub trait Request {
    type Reply;
}

/// Held by the real-time side while it runs one request. Goes back on
/// `returned` once used, never dropped there: it may be the last end of its
/// ring.
pub struct ReplySender<R> {
    reply: Producer<R>,
}

/// The caller's end of one request's reply ring. Dropping it early is
/// safe: the real-time side's end comes back, and the ring is freed here.
pub struct ReplyReceiver<R> {
    reply: Consumer<R>,
    /// Its one reply was taken, so an empty ring no longer means not yet.
    taken: bool,
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
    requests: Producer<(Q, ReplySender<Q::Reply>)>,
    /// Reply ends the real-time side is done with, to drop here.
    returned: Consumer<ReplySender<Q::Reply>>,
    /// Opened and not yet back on `returned`; `open` refuses at capacity.
    in_flight: usize,
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

pub struct RealtimeEnd<Q: Request> {
    requests: Consumer<(Q, ReplySender<Q::Reply>)>,
    returned: Producer<ReplySender<Q::Reply>>,
}

impl<Q: Request> RealtimeEnd<Q> {
    pub fn try_recv(&mut self) -> Option<(Q, ReplySender<Q::Reply>)> {
        self.requests.pop().ok()
    }
}

/// `capacity` is in requests: the most that can be in flight at once,
/// from open until their reply end is back. Both rings hold that, as each
/// request sends one open and returns one end, so neither ever fills.
pub fn channel<Q: Request>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    let (requests_producer, requests_consumer) = RingBuffer::new(capacity);
    let (returned_producer, returned_consumer) = RingBuffer::new(capacity);
    let control = ControlEnd {
        requests: requests_producer,
        returned: returned_consumer,
        in_flight: 0,
    };
    let realtime = RealtimeEnd {
        requests: requests_consumer,
        returned: returned_producer,
    };
    (control, realtime)
}
