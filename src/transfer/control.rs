//! The control side's end, and each value's transfer. Allocating and
//! freeing are fine here.

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use super::Completion;

/// The caller's claim on one value while it is away. Dropping it early is
/// safe: the value and its ring still come back, and are freed here.
#[derive(Debug)]
pub struct Transfer<T> {
    pub(super) ring: Consumer<T>,
    /// Its value was taken back, so an empty ring no longer means not yet.
    pub(super) taken: bool,
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
    pub(super) started: Producer<Completion<T>>,
    pub(super) returned: Consumer<Producer<T>>,
    /// Started and not yet back on `returned`.
    pub(super) in_flight: usize,
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
