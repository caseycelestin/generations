//! The control side's end, and each value's transfer. Allocating and
//! freeing are fine here.

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use super::Completion;

/// The caller's claim on one value while it is away. Dropping it early is
/// safe: the completion still comes back, and the value is freed here.
#[derive(Debug)]
pub struct Transfer<T> {
    pub(super) ring: Consumer<T>,
    /// Its value was taken back, so an empty ring no longer means not yet.
    pub(super) taken: bool,
}

impl<T> Transfer<T> {
    /// A value that came back before its completion was dropped is still
    /// taken.
    pub fn try_wait(&mut self) -> Result<T, TryWaitError> {
        if self.taken {
            return Err(TryWaitError::Taken);
        }
        // Asked before popping: a completion dropped by now pushed any value
        // first, so an empty ring after it means none is coming.
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

/// One deep: a value comes back once.
fn return_ring<T>() -> (Completion<T>, Transfer<T>) {
    let (producer, consumer) = RingBuffer::new(1);
    let completion = Completion { ring: producer };
    let transfer = Transfer {
        ring: consumer,
        taken: false,
    };
    (completion, transfer)
}

pub struct ControlEnd<T> {
    pub(super) started: Producer<(T, Completion<T>)>,
    pub(super) returned: Consumer<Completion<T>>,
    /// Started and not yet back on `returned`.
    pub(super) in_flight: usize,
}

impl<T> ControlEnd<T> {
    /// Hands the value back when `capacity` transfers are in flight.
    pub fn try_start(&mut self, value: T) -> Result<Transfer<T>, T> {
        self.take_in();
        if self.in_flight == self.started.buffer().capacity() {
            return Err(value);
        }
        let (completion, transfer) = return_ring();
        let pushed = self.started.push((value, completion));
        assert!(pushed.is_ok(), "the ring holds every transfer in flight");
        self.in_flight += 1;
        Ok(transfer)
    }

    /// Rings whose transfer is gone are freed here, with any value in them.
    pub fn take_in(&mut self) {
        while let Ok(completion) = self.returned.pop() {
            drop(completion);
            self.in_flight -= 1;
        }
    }
}
