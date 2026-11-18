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
    /// Each slot's reply ring.
    replies: Vec<(Slot, Consumer<Q::Reply>)>,
}

impl<Q: Request> ControlEnd<Q> {
    /// Hands the request back when every slot is open.
    pub fn open(&mut self, request: Q) -> Result<RequestHandle<Q>, Q> {
        let Some((slot, reply)) = self.replies.pop() else {
            return Err(request);
        };
        let pushed = self.to_realtime.push((slot, request));
        assert!(pushed.is_ok(), "the ring holds a request per slot");
        Ok(RequestHandle { slot, reply })
    }
}

/// Owns its slot's reply ring while its request is open.
pub struct RequestHandle<Q: Request> {
    slot: Slot,
    reply: Consumer<Q::Reply>,
}

pub struct RealtimeEnd<Q: Request> {
    from_control: Consumer<(Slot, Q)>,
    /// By slot.
    replies: Box<[Producer<Q::Reply>]>,
}

/// `capacity` is in requests: the most that can be open at once. The ring
/// of opens holds that, and each slot's reply ring one, as a slot carries
/// one request and one reply at a time, so none ever fills.
pub fn channel<Q: Request>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
    let (to_realtime, from_control) = RingBuffer::new(capacity);
    let (to_control, from_realtime): (Vec<_>, Vec<_>) =
        (0..capacity).map(|_| RingBuffer::new(1)).unzip();
    let control = ControlEnd {
        to_realtime,
        replies: from_realtime
            .into_iter()
            .enumerate()
            .map(|(index, reply)| (Slot(index), reply))
            .collect(),
    };
    let realtime = RealtimeEnd {
        from_control,
        replies: to_control.into_boxed_slice(),
    };
    (control, realtime)
}
