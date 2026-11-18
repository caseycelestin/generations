//! Requests from control-side callers to a real-time side that only open
//! and return: each is answered by exactly one reply, and nothing else
//! crosses. Each request brings its own reply ring, made on the control
//! side; the real-time side sends that ring's end back once it has
//! replied, so it is freed on the control side. Knows nothing of audio.
//!
//! Each side's end is its own module, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{ControlEnd, ReplyReceiver, TryWaitError};
pub use realtime::{RealtimeEnd, ReplySender};

use rtrb::RingBuffer;

/// Pairs a request type with its reply.
pub trait Request {
    type Reply;
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
