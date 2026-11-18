//! Requests from control-side callers to a real-time side, each answered
//! by exactly one reply; nothing else crosses. Each request brings its own
//! reply ring, made on the control side, and the real-time side sends its
//! end back after replying, so the ring is freed on the control side.
//! Knows nothing of audio.
//!
//! Each side's end is its own module, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{ControlEnd, ReplyReceiver, TryWaitError};
pub use realtime::{RealtimeEnd, ReplySender};

use rtrb::RingBuffer;

/// `capacity` is how many requests can be in flight, from sent until their
/// `ReplySender` is back; both rings hold that many, so neither ever fills.
pub fn channel<Q>(capacity: usize) -> (ControlEnd<Q>, RealtimeEnd<Q>) {
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

    /// A request the real-time side answers by changing its number.
    #[derive(Debug, PartialEq)]
    struct Ask(u32);

    /// A request that carries something whose freeing can be watched.
    #[derive(Debug)]
    struct Carry(#[expect(dead_code, reason = "held only to be dropped")] Arc<()>);

    #[test]
    fn a_request_comes_back_to_its_receiver_with_its_answer() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(7)).unwrap();
        let (Ask(number), sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, Ask(number * 2));
        assert_eq!(receiver.try_wait(), Ok(Ask(14)));
    }

    #[test]
    fn each_request_comes_back_to_its_own_receiver() {
        let (mut control, mut realtime) = channel(2);
        let mut first = control.try_send(Ask(1)).unwrap();
        let mut second = control.try_send(Ask(2)).unwrap();
        let (first_request, first_sender) = realtime.try_recv().unwrap();
        let (second_request, second_sender) = realtime.try_recv().unwrap();
        realtime.reply(second_sender, second_request);
        realtime.reply(first_sender, first_request);
        assert_eq!(first.try_wait(), Ok(Ask(1)));
        assert_eq!(second.try_wait(), Ok(Ask(2)));
    }

    #[test]
    fn sending_is_refused_at_capacity_until_a_sender_is_back() {
        let (mut control, mut realtime) = channel(1);
        let _receiver = control.try_send(Ask(1)).unwrap();
        assert_eq!(control.try_send(Ask(2)).err(), Some(Ask(2)));

        let (request, sender) = realtime.try_recv().unwrap();
        assert_eq!(
            control.try_send(Ask(3)).err(),
            Some(Ask(3)),
            "a request read but not answered is still in flight"
        );

        realtime.reply(sender, request);
        assert!(control.try_send(Ask(4)).is_ok());
    }

    #[test]
    fn a_channel_of_no_capacity_refuses_every_request() {
        let (mut control, _realtime) = channel(0);
        assert_eq!(control.try_send(Ask(1)).err(), Some(Ask(1)));
    }

    #[test]
    fn try_wait_is_empty_until_the_request_comes_back() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (request, sender) = realtime.try_recv().unwrap();
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Empty));
        realtime.reply(sender, request);
        assert_eq!(receiver.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn a_request_is_taken_back_once() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (request, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, request);
        assert_eq!(receiver.try_wait(), Ok(Ask(1)));
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Taken));
    }

    #[test]
    fn a_request_whose_sender_was_dropped_since_is_still_taken() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (request, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, request);
        control.take_in();
        assert_eq!(receiver.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn a_sender_dropped_without_replying_disconnects_its_receiver() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        let (_, sender) = realtime.try_recv().unwrap();
        drop(sender);
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_request_never_read_disconnects_once_the_realtime_end_is_gone() {
        let (mut control, realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        drop(realtime);
        assert_eq!(receiver.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_dropped_receivers_request_is_freed_on_the_control_side() {
        let (mut control, mut realtime) = channel(1);
        let carried = Arc::new(());
        drop(control.try_send(Carry(Arc::clone(&carried))).unwrap());
        let (request, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, request);
        assert_eq!(Arc::strong_count(&carried), 2, "in its ring, not yet freed");

        control.take_in();

        assert_eq!(Arc::strong_count(&carried), 1);
    }

    #[test]
    fn a_full_channel_can_be_filled_again_once_every_sender_is_back() {
        let (mut control, mut realtime) = channel(2);
        for round in 0..3 {
            let _first = control.try_send(Ask(round)).unwrap();
            let _second = control.try_send(Ask(round)).unwrap();
            assert!(
                control.try_send(Ask(round)).is_err(),
                "full in round {round}"
            );
            while let Some((request, sender)) = realtime.try_recv() {
                realtime.reply(sender, request);
            }
        }
    }

    #[test]
    fn the_realtime_end_still_replies_once_the_control_end_is_gone() {
        let (mut control, mut realtime) = channel(1);
        let mut receiver = control.try_send(Ask(1)).unwrap();
        drop(control);
        let (request, sender) = realtime.try_recv().unwrap();
        realtime.reply(sender, request);
        assert_eq!(receiver.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn an_unread_request_is_dropped_with_the_realtime_end() {
        let (mut control, realtime) = channel(1);
        let carried = Arc::new(());
        let _receiver = control.try_send(Carry(Arc::clone(&carried))).unwrap();
        assert_eq!(Arc::strong_count(&carried), 2, "on the ring");

        drop(realtime);

        assert_eq!(
            Arc::strong_count(&carried),
            1,
            "dropped though the control end still holds the ring"
        );
    }
}
