//! Owned values handed by control-side callers to a real-time side and
//! back; nothing else crosses. The real-time side works on each value, then
//! completes it, and it comes back with whatever was written into it, so
//! everything it owns is freed on the control side. Each transfer brings
//! its own return ring, whose sending end comes back after the value, so
//! the ring is freed there as well. Knows nothing of audio.

use std::ops::{Deref, DerefMut};

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

/// `capacity` is how many transfers can be in flight, from started until
/// their return ring's sending end is back; both rings hold that many, so
/// neither ever fills.
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

/// The caller's claim on one value while it is away. Dropping it early is
/// safe: the value and its ring still come back, and are freed here.
#[derive(Debug)]
pub struct Transfer<T> {
    ring: Consumer<T>,
    /// Its value was taken back, so an empty ring no longer means not yet.
    taken: bool,
}

impl<T> Transfer<T> {
    /// A value that came back before the ring's other end was freed is
    /// still taken.
    pub fn try_wait(&mut self) -> Result<T, TryWaitError> {
        if self.taken {
            return Err(TryWaitError::Taken);
        }
        // Asked before popping: the other end, if freed by now, pushed any
        // value first, so an empty ring after it means none is coming.
        let abandoned = self.ring.is_abandoned();
        let Ok(value) = self.ring.pop() else {
            if abandoned {
                return Err(TryWaitError::Disconnected);
            }
            return Err(TryWaitError::Empty);
        };
        self.taken = true;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryWaitError {
    #[error("the value is not back yet")]
    Empty,
    #[error("the value was already taken back")]
    Taken,
    /// Its completion was dropped without completing, as when the
    /// real-time side is gone.
    #[error("the value will not come back")]
    Disconnected,
}

#[derive(Debug)]
pub struct ControlEnd<T> {
    started: Producer<Completion<T>>,
    returned: Consumer<Producer<T>>,
    /// Started and not yet back on `returned`.
    in_flight: usize,
}

/// Nothing was started; the value comes back with the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StartError<T> {
    /// `capacity` transfers are in flight.
    #[error("every transfer is in flight")]
    Full(T),
    /// Nothing would ever receive it.
    #[error("the real-time end is gone")]
    Disconnected(T),
}

impl<T> ControlEnd<T> {
    pub fn try_start(&mut self, value: T) -> Result<Transfer<T>, StartError<T>> {
        if self.started.is_abandoned() {
            return Err(StartError::Disconnected(value));
        }
        self.take_in();
        if self.in_flight == self.started.buffer().capacity() {
            return Err(StartError::Full(value));
        }
        // One deep: a value comes back once.
        let (producer, consumer) = RingBuffer::new(1);
        let transfer = Transfer {
            ring: consumer,
            taken: false,
        };
        let completion = Completion {
            value,
            ring: producer,
        };
        let pushed = self.started.push(completion);
        assert!(pushed.is_ok(), "the ring holds every transfer in flight");
        self.in_flight += 1;
        Ok(transfer)
    }

    /// Rings whose transfer is gone are freed here, with any value in them.
    pub fn take_in(&mut self) {
        while let Ok(ring) = self.returned.pop() {
            drop(ring);
            self.in_flight -= 1;
        }
    }
}

/// A received value with its way home. Never dropped on the real-time side,
/// as its value may own memory and its ring may be the last end: `complete`
/// sends both back.
#[derive(Debug)]
pub struct Completion<T> {
    value: T,
    ring: Producer<T>,
}

/// The value is read and written through it, never moved out: it leaves
/// only through `complete`.
impl<T> Deref for Completion<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Completion<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

/// Wait-free and bounded, as is each `Completion`: never allocates, frees,
/// locks or blocks, except when dropped, at teardown, when no deadline is
/// left.
#[derive(Debug)]
pub struct RealtimeEnd<T> {
    started: Consumer<Completion<T>>,
    returned: Producer<Producer<T>>,
}

/// Drops every value not yet received, so their transfers learn it will not
/// come back: the ring keeps them while the control end lives.
impl<T> Drop for RealtimeEnd<T> {
    fn drop(&mut self) {
        while self.started.pop().is_ok() {}
    }
}

impl<T> RealtimeEnd<T> {
    pub fn try_recv(&mut self) -> Option<Completion<T>> {
        self.started.pop().ok()
    }

    /// Takes the completion, so a transfer completes once and its value and
    /// ring always go back, to be freed on the control side. Cannot fail:
    /// each ring has room for it.
    pub fn complete(&mut self, completion: Completion<T>) {
        let Completion { value, mut ring } = completion;
        let sent = ring.push(value);
        debug_assert!(sent.is_ok(), "a return ring holds its one value");
        let returned = self.returned.push(ring);
        debug_assert!(returned.is_ok(), "the ring holds every transfer in flight");
    }
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
