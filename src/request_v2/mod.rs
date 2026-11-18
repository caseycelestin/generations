//! Requests from control-side callers to a real-time side that are only
//! sent and answered: each is answered by exactly one reply, and nothing
//! else crosses. Each request brings its own reply ring, made on the control
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
/// from sent until their `ReplySender` is back. Both rings hold that, as
/// each request goes out once and its sender comes back once, so neither
/// ever fills.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A request carrying a number, answered with a number.
    #[derive(Debug)]
    struct Ask(u32);

    impl Request for Ask {
        type Reply = u32;
    }

    /// A request whose reply can be watched being freed.
    #[derive(Debug)]
    struct Hold;

    impl Request for Hold {
        type Reply = Arc<()>;
    }

    #[test]
    fn a_reply_reaches_the_receiver_of_its_request() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(7)).unwrap();
        let (Ask(number), sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, number * 2);
        assert_eq!(receiver.try_wait(), Ok(14));
    }

    #[test]
    fn each_reply_reaches_its_own_receiver() {
        let (mut control, mut realtime) = channel::<Ask>(2);
        let mut first = control.try_send(Ask(1)).unwrap();
        let mut second = control.try_send(Ask(2)).unwrap();
        let (_, first_sender) = realtime.try_recv().unwrap();
        let (_, second_sender) = realtime.try_recv().unwrap();
        realtime.reply(second_sender, 20);
        realtime.reply(first_sender, 10);
        assert_eq!(first.try_wait(), Ok(10));
        assert_eq!(second.try_wait(), Ok(20));
    }

    #[test]
    fn sending_is_refused_at_capacity_until_a_sender_is_back() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let _receiver = control.try_send(Ask(1)).unwrap();
        assert!(matches!(control.try_send(Ask(2)), Err(Ask(2))));

        let (_, sender) = realtime.try_recv().unwrap();
        assert!(
            matches!(control.try_send(Ask(3)), Err(Ask(3))),
            "a request read but not answered is still in flight"
        );

        realtime.reply(sender, 0);
        assert!(control.try_send(Ask(4)).is_ok());
    }

    #[test]
    fn a_channel_of_no_capacity_refuses_every_request() {
        let (mut control, _realtime) = channel::<Ask>(0);
        assert!(matches!(control.try_send(Ask(1)), Err(Ask(1))));
    }

    #[test]
    fn try_wait_is_empty_until_the_reply_arrives() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (_, sender) = realtime.try_recv().unwrap();
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Empty));
        realtime.reply(sender, 1);
        assert_eq!(receiver.try_wait(), Ok(1));
    }

    #[test]
    fn a_reply_is_taken_once() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (_, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, 1);
        assert_eq!(receiver.try_wait(), Ok(1));
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Taken));
    }

    #[test]
    fn a_reply_whose_sender_was_dropped_since_is_still_taken() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (_, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, 1);
        control.take_in();
        assert_eq!(receiver.try_wait(), Ok(1));
    }

    #[test]
    fn a_sender_dropped_without_replying_disconnects_its_receiver() {
        let (mut control, mut realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (_, sender) = realtime.try_recv().unwrap();
        drop(sender);
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_request_never_read_disconnects_once_the_realtime_end_is_gone() {
        let (mut control, realtime) = channel::<Ask>(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        drop(realtime);
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_dropped_receivers_reply_is_freed_on_the_control_side() {
        let (mut control, mut realtime) = channel::<Hold>(1);
        let held = Arc::new(());
        drop(control.try_send(Hold).unwrap());
        let (Hold, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, Arc::clone(&held));
        assert_eq!(Arc::strong_count(&held), 2, "in its ring, not yet freed");

        control.take_in();

        assert_eq!(Arc::strong_count(&held), 1);
    }
}
