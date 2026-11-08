//! Recording: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this turns periods of input into a buffer
//! the engine can store. The rules are the render module's: no allocation,
//! no lock, no disk, no error path, no clock, because the [`Recorder`] runs
//! wherever the frames come from, live on the audio thread or offline in a
//! loop, and must not care which.
//!
//! ```text
//! control side                                audio side
//! (the engine)  mints ──▶  Take     ◀── one-slot ring ──  Recorder
//!                          holds the memory              fills the buffer,
//!                          and closes the take           hands it back once
//! ```
//!
//! A take is opened through the engine, which reserves and allocates the
//! whole buffer up front and returns the two halves. The caller moves the
//! recorder to where the frames are and keeps the take. The recorder
//! finishes when its buffer fills or when it is dropped, and either way
//! hands the buffer to the take over the ring; nothing travels the other
//! way. Closing the take, again through the engine, stores what came back
//! as a new root. Dropping it unclosed releases the memory and the take is
//! gone.

// Nothing reads these fields until `write` and `close` arrive; so far a
// take can only be opened.
#![allow(dead_code)]

use std::fmt;

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError, Dither, check_channels};
use crate::config::Pcm;
use crate::ledger::{Reservation, ShortReservation};

/// Reasons a take cannot be opened.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RecordError {
    /// The channel count is not one a buffer may have.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// A take of no frames was asked for.
    #[error("a take must be at least one frame long")]
    Empty,
    /// The reservation offered does not pay for the take's buffer. Nothing
    /// was allocated.
    #[error("opening a take")]
    ShortReservation(#[from] ShortReservation),
}

/// Bytes a take of `frames` frames of `channels` channels will hold, for
/// reserving before [`open`]. Refuses a shape no buffer can have, so a
/// bad request is caught before anything is reserved for it.
pub(crate) fn byte_cost(channels: u16, frames: usize) -> Result<usize, RecordError> {
    check_channels(channels)?;
    if frames == 0 {
        return Err(RecordError::Empty);
    }
    Ok(AudioBuffer::bytes_for(channels, frames))
}

/// Opens a take of `frames` frames of `channels` channels, paid for by
/// `reservation`, and returns its two halves.
///
/// This is the take's one allocation: the whole buffer, at full capacity
/// and empty, so that nothing the recorder does later needs memory it
/// does not have. `reservation` must cover [`byte_cost`]; a short one is
/// refused before anything is allocated. Unlike the crate's other
/// allocators, this takes the reservation rather than borrowing it,
/// because the take is what holds it: the bytes belong to the recording
/// until the take is closed or dropped, and the engine keeps no record of
/// either.
pub(crate) fn open(
    channels: u16,
    frames: usize,
    keep_loaded: bool,
    reservation: Reservation,
) -> Result<(Take, Recorder), RecordError> {
    reservation.can_hold(byte_cost(channels, frames)?)?;
    let (producer, consumer) = RingBuffer::new(1);
    let recorder = Recorder {
        data: Vec::with_capacity(frames * usize::from(channels)),
        channels,
        dither: Dither::default(),
        finished: Some(producer),
    };
    let take = Take {
        reservation,
        channels,
        keep_loaded,
        finished: consumer,
    };
    Ok((take, recorder))
}

/// The audio side of a take: fills a buffer that already exists.
///
/// Moved to wherever the frames come from and fed each period. Finishes
/// when the buffer is full or when it is dropped, and either way hands
/// the buffer to its [`Take`] over the ring. Writing to a finished
/// recorder does nothing.
pub struct Recorder {
    /// The take's audio, interleaved, allocated to full capacity when the
    /// take was opened. The length is how much has been written and the
    /// capacity is the take's length. Never grows.
    data: Vec<Pcm>,
    /// Channels per frame, one or two.
    channels: u16,
    /// Quantiser state, so every sample is dithered as it lands and the
    /// buffer is in the project format from the first frame.
    dither: Dither,
    /// The one-slot ring back to the take. `None` once the buffer has
    /// been handed over, which is how the recorder knows it is finished.
    finished: Option<Producer<Vec<Pcm>>>,
}

impl fmt::Debug for Recorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Recorder")
            .field("channels", &self.channels)
            .field("frames", &(self.data.len() / usize::from(self.channels)))
            .field(
                "capacity",
                &(self.data.capacity() / usize::from(self.channels)),
            )
            .field("finished", &self.finished.is_none())
            .finish_non_exhaustive()
    }
}

/// The control side of a take: the memory it paid for and the way the
/// audio comes back.
///
/// Closed through the engine, which stores the audio as a new root.
/// Dropped unclosed, it releases the memory and the take is gone.
pub struct Take {
    /// The bytes the buffer costs, held until the take is closed or
    /// dropped. The recorder's buffer is what it pays for.
    reservation: Reservation,
    /// Channels per frame, one or two; the frame count of what comes
    /// back is its length divided by this.
    channels: u16,
    /// Whether the stored sample stays in RAM as its loaded copy, rather
    /// than being dropped once it is on disk.
    keep_loaded: bool,
    /// The receiving end of the ring. Empty until the recorder finishes.
    finished: Consumer<Vec<Pcm>>,
}

impl fmt::Debug for Take {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Take")
            .field("channels", &self.channels)
            .field("reservation", &self.reservation)
            .field("keep_loaded", &self.keep_loaded)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use crate::testing::paid;

    #[test]
    fn byte_cost_is_the_buffer_the_take_will_hold() {
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
            open(0, 10, false, paid()).unwrap_err(),
            RecordError::Audio(_)
        ));
        assert_eq!(open(1, 0, false, paid()).unwrap_err(), RecordError::Empty);
    }

    #[test]
    fn open_allocates_the_whole_take_empty() {
        let (take, recorder) = open(2, 10, true, paid()).unwrap();
        assert_eq!(recorder.data.capacity(), 20);
        assert!(recorder.data.is_empty());
        assert_eq!(recorder.channels, 2);
        assert!(recorder.finished.is_some());
        assert_eq!(take.channels, 2);
        assert!(take.keep_loaded);
        assert!(take.finished.is_empty());
    }

    #[test]
    fn open_refuses_a_reservation_short_of_the_buffer() {
        let ledger = Ledger::new(usize::MAX);
        let needed = byte_cost(1, 10).unwrap();
        assert!(open(1, 10, false, ledger.reserve(needed).unwrap()).is_ok());
        assert!(matches!(
            open(1, 10, false, ledger.reserve(needed - 1).unwrap()).unwrap_err(),
            RecordError::ShortReservation(ShortReservation { needed: n, reserved: r })
                if n == needed && r == needed - 1
        ));
    }
}
