//! Owned values handed by control-side callers to a real-time side and
//! back; nothing else crosses. The real-time side works on each value, then
//! completes it, and it comes back with whatever was written into it, so
//! everything it owns is freed on the control side. Each transfer brings
//! its own return ring, and its completion comes back too, so the ring is
//! freed there as well. Knows nothing of audio.
//!
//! Each side's end is its own module, so `realtime.rs` can be held to the
//! real-time rules alone.

mod control;
mod realtime;

pub use control::{ControlEnd, StartError, Transfer, TryWaitError};
pub use realtime::{Completion, RealtimeEnd};

use rtrb::RingBuffer;

/// `capacity` is how many transfers can be in flight, from started until
/// their `Completion` is back; both rings hold that many, so neither ever
/// fills.
pub fn channel<T>(capacity: usize) -> (ControlEnd<T>, RealtimeEnd<T>) {
    let (started_producer, started_consumer) = RingBuffer::new(capacity);
    let (returned_producer, returned_consumer) = RingBuffer::new(capacity);
    let control = ControlEnd {
        started: started_producer,
        returned: returned_consumer,
        in_flight: 0,
    };
    let realtime = RealtimeEnd {
        started: started_consumer,
        returned: returned_producer,
    };
    (control, realtime)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A value the real-time side works on by changing its number.
    #[derive(Debug, PartialEq)]
    struct Ask(u32);

    /// A value that carries something whose freeing can be watched.
    #[derive(Debug)]
    struct Carry(#[expect(dead_code, reason = "held only to be dropped")] Arc<()>);

    #[test]
    fn a_value_comes_back_to_its_transfer_with_what_was_written() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(7)).unwrap();
        let mut completion = realtime.try_recv().unwrap();
        completion.0 *= 2;
        realtime.complete(completion);
        assert_eq!(transfer.try_wait(), Ok(Ask(14)));
    }

    #[test]
    fn each_value_comes_back_to_its_own_transfer() {
        let (mut control, mut realtime) = channel(2);
        let mut first = control.try_start(Ask(1)).unwrap();
        let mut second = control.try_start(Ask(2)).unwrap();
        let first_completion = realtime.try_recv().unwrap();
        let second_completion = realtime.try_recv().unwrap();
        realtime.complete(second_completion);
        realtime.complete(first_completion);
        assert_eq!(first.try_wait(), Ok(Ask(1)));
        assert_eq!(second.try_wait(), Ok(Ask(2)));
    }

    #[test]
    fn starting_is_refused_at_capacity_until_a_completion_is_back() {
        let (mut control, mut realtime) = channel(1);
        let _transfer = control.try_start(Ask(1)).unwrap();
        assert_eq!(
            control.try_start(Ask(2)).err(),
            Some(StartError::Full(Ask(2)))
        );

        let completion = realtime.try_recv().unwrap();
        assert_eq!(
            control.try_start(Ask(3)).err(),
            Some(StartError::Full(Ask(3))),
            "a value received but not completed is still in flight"
        );

        realtime.complete(completion);
        assert!(control.try_start(Ask(4)).is_ok());
    }

    #[test]
    fn a_channel_of_no_capacity_refuses_every_transfer() {
        let (mut control, _realtime) = channel(0);
        assert_eq!(
            control.try_start(Ask(1)).err(),
            Some(StartError::Full(Ask(1)))
        );
    }

    #[test]
    fn try_wait_is_empty_until_the_value_comes_back() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        let completion = realtime.try_recv().unwrap();
        assert_eq!(transfer.try_wait(), Err(TryWaitError::Empty));
        realtime.complete(completion);
        assert_eq!(transfer.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn a_value_is_taken_back_once() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        let completion = realtime.try_recv().unwrap();
        realtime.complete(completion);
        assert_eq!(transfer.try_wait(), Ok(Ask(1)));
        assert_eq!(transfer.try_wait(), Err(TryWaitError::Taken));
    }

    #[test]
    fn a_value_whose_completion_was_dropped_since_is_still_taken() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        let completion = realtime.try_recv().unwrap();
        realtime.complete(completion);
        control.take_in();
        assert_eq!(transfer.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn a_completion_dropped_without_completing_disconnects_its_transfer() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        let completion = realtime.try_recv().unwrap();
        drop(completion);
        assert_eq!(transfer.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn starting_is_refused_once_the_realtime_end_is_gone() {
        let (mut control, realtime) = channel(1);
        drop(realtime);
        assert_eq!(
            control.try_start(Ask(1)).err(),
            Some(StartError::Disconnected(Ask(1)))
        );
    }

    #[test]
    fn a_value_never_received_disconnects_once_the_realtime_end_is_gone() {
        let (mut control, realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        drop(realtime);
        assert_eq!(transfer.try_wait(), Err(TryWaitError::Disconnected));
    }

    #[test]
    fn a_dropped_transfers_value_is_freed_on_the_control_side() {
        let (mut control, mut realtime) = channel(1);
        let carried = Arc::new(());
        drop(control.try_start(Carry(Arc::clone(&carried))).unwrap());
        let completion = realtime.try_recv().unwrap();
        realtime.complete(completion);
        assert_eq!(Arc::strong_count(&carried), 2, "in its ring, not yet freed");

        control.take_in();

        assert_eq!(Arc::strong_count(&carried), 1);
    }

    #[test]
    fn a_full_channel_can_be_filled_again_once_every_completion_is_back() {
        let (mut control, mut realtime) = channel(2);
        for round in 0..3 {
            let _first = control.try_start(Ask(round)).unwrap();
            let _second = control.try_start(Ask(round)).unwrap();
            assert!(
                control.try_start(Ask(round)).is_err(),
                "full in round {round}"
            );
            while let Some(completion) = realtime.try_recv() {
                realtime.complete(completion);
            }
        }
    }

    #[test]
    fn the_realtime_end_still_completes_once_the_control_end_is_gone() {
        let (mut control, mut realtime) = channel(1);
        let mut transfer = control.try_start(Ask(1)).unwrap();
        drop(control);
        let completion = realtime.try_recv().unwrap();
        realtime.complete(completion);
        assert_eq!(transfer.try_wait(), Ok(Ask(1)));
    }

    #[test]
    fn an_unreceived_value_is_dropped_with_the_realtime_end() {
        let (mut control, realtime) = channel(1);
        let carried = Arc::new(());
        let _transfer = control.try_start(Carry(Arc::clone(&carried))).unwrap();
        assert_eq!(Arc::strong_count(&carried), 2, "on the ring");

        drop(realtime);

        assert_eq!(
            Arc::strong_count(&carried),
            1,
            "dropped though the control end still holds the ring"
        );
    }
}
