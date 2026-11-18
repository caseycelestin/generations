//! Requests from control-side callers to a real-time side that only open
//! and return: each is answered by exactly one reply, and nothing else
//! crosses. Anything said to a request while it is open, or about it, is
//! for shared cells the caller keeps by slot. Knows nothing of audio.

use rtrb::{Consumer, Producer, RingBuffer};

/// Pairs a request type with its reply.
pub trait Request {
    type Reply;
}

/// Where a request lives while it is open, counted from zero below the
/// channel's capacity. Reused once its reply is taken, so one slot is only
/// ever one open request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Slot(usize);

pub struct ControlEnd<Q: Request> {
    to_realtime: Producer<(Slot, Q)>,
    from_realtime: Consumer<(Slot, Q::Reply)>,
}

pub struct RealtimeEnd<Q: Request> {
    from_control: Consumer<(Slot, Q)>,
    to_control: Producer<(Slot, Q::Reply)>,
}

/// `capacity` is in requests: the most that can be open at once. Both rings
/// hold that, as a slot carries one request and one reply at a time, so
/// neither ever fills.
pub fn channel<Q: Request>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    let (to_realtime, from_control) = RingBuffer::new(capacity);
    let (to_control, from_realtime) = RingBuffer::new(capacity);
    let control = ControlEnd {
        to_realtime,
        from_realtime,
    };
    let realtime = RealtimeEnd {
        from_control,
        to_control,
    };
    (control, realtime)
}
