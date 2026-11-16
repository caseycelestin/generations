//! Requests from control-side callers to a real-time side, each answered by
//! exactly one reply. One request type per channel, so neither end converts
//! what it receives; a feature with several kinds of request makes them one
//! enum. Knows nothing of audio.
//!
//! - On the real-time side every operation is wait-free and bounded: it
//!   never allocates, frees, locks or blocks. The control side may.
//! - Memory is fixed at creation.
//! - A reservation holds room for a request, its cancel, one status
//!   question at a time and the answers to both. Past it, only a second
//!   question while one is out is refused; nothing the real-time side sends
//!   is lost.
//! - Each direction is in order.
//!
//! Each side's end is in its own file, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{
    ControlEnd, PendingStatus, RequestHandle, RequestPermit, ReserveError, StatusError,
    TryRecvStatusError, TryWaitError,
};
pub use realtime::{RealtimeEnd, Received, ReplyPermit, StatusPermit};

use rtrb::RingBuffer;

/// Pairs a request type with its reply and with what it answers while
/// open, when asked its status.
pub trait Request {
    type Reply;
    type Status;
}

/// Unique within a channel and never reused; names, not ages, so they do
/// not order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub(crate) u64);

/// What the ring to the real-time side carries.
#[derive(Debug)]
enum ToRealtime<Q> {
    Request(RequestId, Q),
    Cancel(RequestId),
    Status(RequestId),
}

/// What the ring back carries.
enum FromRealtime<Q: Request> {
    Reply(RequestId, Q::Reply),
    Status(RequestId, Q::Status),
}

/// `capacity` is in requests: the most that can await their reply at once.
/// The ring out holds three times that, for each one's cancel and status
/// question, and the ring back twice, for each one's status answer, so
/// neither ring ever fills.
pub fn channel<Q: Request>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    channel_with_headroom(capacity, 0)
}

/// As `channel`, with the last `headroom` slots of `capacity` kept for
/// `try_reserve_including_headroom`, so ordinary requests cannot crowd it out.
pub fn channel_with_headroom<Q: Request>(
    capacity: usize,
    headroom: usize,
) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    let (to_realtime, from_control) = RingBuffer::new(3 * capacity);
    let (to_control, from_realtime) = RingBuffer::new(2 * capacity);
    let control = ControlEnd::new(to_realtime, from_realtime, capacity, headroom);
    let realtime = RealtimeEnd::new(from_control, to_control);
    (control, realtime)
}
