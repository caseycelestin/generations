//! Recording: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this will turn periods of input into
//! buffers the engine can store. The [`Recorder`] is the control side: the
//! engine mints it, paying for its memory out of the budget as it does, and
//! the caller keeps it. The recorder allocates that memory once, when it is
//! made, so nothing recorded through it ever needs memory it does not
//! already have. The audio side, the take, is not here yet.

use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError, check_channels};
use crate::config::Pcm;
use crate::ledger::{Reservation, ShortReservation};

/// Reasons a recorder cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordError {
    /// The channel count is not one a buffer may have.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// A recorder of no frames was asked for.
    #[error("a recorder must hold at least one frame")]
    Empty,
    /// The reservation offered does not pay for the recorder's buffer.
    /// Nothing was allocated.
    #[error("making a recorder")]
    ShortReservation(#[from] ShortReservation),
}

/// Bytes a recorder of `frames` frames of `channels` channels holds: what
/// the engine reserves before making one. Refuses a shape no buffer can
/// have, so a bad request is caught before anything is reserved for it.
pub(crate) fn byte_cost(channels: u16, frames: usize) -> Result<usize, RecordError> {
    check_channels(channels)?;
    if frames == 0 {
        return Err(RecordError::Empty);
    }
    Ok(AudioBuffer::bytes_for(channels, frames))
}

/// The control side of recording: holds the memory a recording will use
/// and will mint takes into it.
///
/// Minted by the engine, kept by the caller. It never goes to the audio
/// thread. Dropping it gives its memory back.
#[derive(Debug)]
pub struct Recorder {
    /// The recorder's audio memory, interleaved, allocated to full capacity
    /// and empty when the recorder is made. Never grows.
    // Read once takes are minted; until then the recorder only holds it.
    #[allow(dead_code)]
    data: Vec<Pcm>,
    /// Channels per frame, one or two.
    #[allow(dead_code)]
    channels: u16,
    /// The bytes `data` costs, reserved by the engine before the recorder
    /// was made and held for as long as it lives.
    #[allow(dead_code)]
    reservation: Reservation,
}

impl Recorder {
    /// A recorder with room for `frames` frames of `channels` channels,
    /// paid for by `reservation`.
    ///
    /// This is the recorder's one allocation. `reservation` must cover
    /// [`byte_cost`]; a short one is refused before anything is allocated.
    /// Unlike the crate's other allocators, this takes the reservation
    /// rather than borrowing it, because the recorder is what holds the
    /// memory: the bytes are its until it is dropped.
    pub(crate) fn new(
        channels: u16,
        frames: usize,
        reservation: Reservation,
    ) -> Result<Self, RecordError> {
        reservation.can_hold(byte_cost(channels, frames)?)?;
        Ok(Self {
            data: Vec::with_capacity(frames * usize::from(channels)),
            channels,
            reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use crate::testing::paid;

    #[test]
    fn byte_cost_is_the_buffer_the_recorder_will_hold() {
        assert_eq!(byte_cost(1, 10), Ok(AudioBuffer::bytes_for(1, 10)));
        assert_eq!(byte_cost(2, 10), Ok(AudioBuffer::bytes_for(2, 10)));
    }

    #[test]
    fn a_shape_no_buffer_can_have_is_refused_before_it_costs_anything() {
        assert_eq!(
            byte_cost(0, 10),
            Err(RecordError::Audio(AudioError::BadChannelCount(0)))
        );
        assert_eq!(
            byte_cost(3, 10),
            Err(RecordError::Audio(AudioError::BadChannelCount(3)))
        );
        assert_eq!(byte_cost(1, 0), Err(RecordError::Empty));
        assert!(matches!(
            Recorder::new(3, 10, paid()).unwrap_err(),
            RecordError::Audio(_)
        ));
        assert_eq!(Recorder::new(1, 0, paid()).unwrap_err(), RecordError::Empty);
    }

    #[test]
    fn new_allocates_the_whole_buffer_empty() {
        let r = Recorder::new(2, 10, paid()).unwrap();
        assert_eq!(r.data.capacity(), 20);
        assert!(r.data.is_empty());
        assert_eq!(r.channels, 2);
    }

    #[test]
    fn new_refuses_a_reservation_short_of_the_buffer() {
        let ledger = Ledger::new(usize::MAX);
        let needed = byte_cost(1, 10).unwrap();
        assert!(Recorder::new(1, 10, ledger.reserve(needed).unwrap()).is_ok());
        assert!(matches!(
            Recorder::new(1, 10, ledger.reserve(needed - 1).unwrap()).unwrap_err(),
            RecordError::ShortReservation(ShortReservation { needed: n, reserved: r })
                if n == needed && r == needed - 1
        ));
    }
}
