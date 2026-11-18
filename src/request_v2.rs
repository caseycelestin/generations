//! Requests from control-side callers to a real-time side that only open
//! and return: each is answered by exactly one reply, and nothing else
//! crosses. Each request brings its own reply ring, made on the control
//! side; the real-time side sends that ring's end back once it has
//! replied, so it is freed on the control side. Knows nothing of audio.

use rtrb::{Consumer, Producer, RingBuffer};

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
}

pub struct ControlEnd<Q: Request> {
    /// Each request with the end its reply goes into.
    requests: Producer<(Q, ReplySender<Q::Reply>)>,
    /// Reply ends the real-time side is done with, to drop here.
    returned: Consumer<ReplySender<Q::Reply>>,
    /// Opened and not yet back on `returned`; `open` refuses at capacity.
    in_flight: usize,
}

pub struct RealtimeEnd<Q: Request> {
    requests: Consumer<(Q, ReplySender<Q::Reply>)>,
    returned: Producer<ReplySender<Q::Reply>>,
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
