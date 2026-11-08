//! Recording: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this will turn periods of input into
//! buffers the engine can store. The [`Recorder`] is the control side: the
//! engine mints it, paying for its memory out of the budget as it does, and
//! the caller keeps it. The recorder allocates that memory once, when it is
//! made, so nothing recorded through it ever needs memory it does not
//! already have.
//!
//! The audio side is the [`Take`]: the recorder's buffer, moved to wherever
//! the frames come from, live on the audio thread or offline in a loop,
//! with the render module's rules: no allocation, no lock, no disk, no
//! error path, no clock. A take is finished when it is dropped, and
//! finishing hands the buffer back to the recorder over a one-slot ring.
//! Nothing travels the other way.
//!
//! ```text
//! control side                                audio side
//! Recorder  ── take() moves the buffer ──▶   Take
//!           ◀── one-slot ring, on drop ────  (fills it, hands it back)
//! ```
//!
//! One buffer, so one take at a time: asking for a take while the last is
//! still out is refused, and asking for one after the last has come back
//! starts over, discarding what it recorded unless it was stored first.

use std::fmt;

use rtrb::{Consumer, Producer, RingBuffer};
use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError, Dither, check_channels};
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
    /// The last take is still out; its buffer has not come back.
    #[error("the last take has not finished")]
    Busy,
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

/// The control side of recording: holds the memory a recording uses and
/// mints takes into it.
///
/// Minted by the engine, kept by the caller. It never goes to the audio
/// thread. Dropping it gives its memory back.
#[derive(Debug)]
pub struct Recorder {
    /// The recorder's audio memory, interleaved, allocated to full capacity
    /// when the recorder is made. Never grows. `None` while a take has it;
    /// its length is then what the last take recorded.
    data: Option<Vec<Pcm>>,
    /// Channels per frame, one or two.
    channels: u16,
    /// The bytes the buffer costs, reserved by the engine before the
    /// recorder was made and held for as long as it lives.
    // Read once a take is stored; until then the recorder only holds it.
    #[allow(dead_code)]
    reservation: Reservation,
    /// The receiving end of the ring the live take will hand its buffer
    /// back on. `None` when no take is out.
    finished: Option<Consumer<Vec<Pcm>>>,
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
            data: Some(Vec::with_capacity(frames * usize::from(channels))),
            channels,
            reservation,
            finished: None,
        })
    }

    /// Whether a take is out: alive somewhere, or finished and not yet
    /// collected. While it is, [`Recorder::take`] is refused.
    pub fn is_recording(&mut self) -> bool {
        self.collect();
        self.data.is_none()
    }

    /// A take: the recorder's buffer, empty, ready to be moved to where
    /// the frames are and filled.
    ///
    /// Refused with [`RecordError::Busy`] while a take is out, before
    /// anything changes; there is one buffer and the live take has it.
    /// [`Recorder::is_recording`] says so without asking. Once the take
    /// has come back, a new one starts over and what the last recorded is
    /// gone, so store it first if it is wanted. The ring the take will
    /// hand the buffer back on is made here, on the control side; it is
    /// the one allocation a take costs and it is not in proportion to
    /// audio length.
    pub fn take(&mut self) -> Result<Take, RecordError> {
        if self.is_recording() {
            return Err(RecordError::Busy);
        }
        let mut data = self
            .data
            .take()
            .expect("not recording means the buffer is here");
        data.clear();
        let (producer, consumer) = RingBuffer::new(1);
        self.finished = Some(consumer);
        Ok(Take {
            data,
            channels: self.channels,
            dither: Dither::default(),
            finished: Some(producer),
        })
    }

    /// Brings the buffer back if the live take has handed it over.
    fn collect(&mut self) {
        if let Some(finished) = &mut self.finished
            && let Ok(data) = finished.pop()
        {
            self.data = Some(data);
            self.finished = None;
        }
    }
}

/// The audio side of a take: fills the recorder's buffer, wherever the
/// frames are.
///
/// Finished by being dropped, which hands the buffer back to the recorder.
/// Dropping the stream that holds it is how a live take ends.
pub struct Take {
    /// The recorder's buffer. The length is how much has been written and
    /// the capacity is the recorder's size. Never grows.
    #[allow(dead_code)]
    data: Vec<Pcm>,
    /// Channels per frame, one or two.
    channels: u16,
    /// Quantiser state, so every sample is dithered as it lands and the
    /// buffer is in the project format from the first frame.
    #[allow(dead_code)]
    dither: Dither,
    /// The one-slot ring back to the recorder. `None` once the buffer has
    /// been handed over.
    finished: Option<Producer<Vec<Pcm>>>,
}

impl Drop for Take {
    /// Hands the buffer back. If the recorder is gone the push finds no
    /// receiver and the buffer is freed here instead, wherever here is;
    /// the roadmap lists the free on the audio thread with the renderer's.
    fn drop(&mut self) {
        if let Some(mut finished) = self.finished.take() {
            // `mem::take` leaves an empty vector that allocates nothing.
            let _ = finished.push(std::mem::take(&mut self.data));
        }
    }
}

impl fmt::Debug for Take {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Take")
            .field("channels", &self.channels)
            .field("frames", &(self.data.len() / usize::from(self.channels)))
            .field(
                "capacity",
                &(self.data.capacity() / usize::from(self.channels)),
            )
            .finish_non_exhaustive()
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
        let data = r.data.as_ref().unwrap();
        assert_eq!(data.capacity(), 20);
        assert!(data.is_empty());
        assert_eq!(r.channels, 2);
    }

    #[test]
    fn a_take_is_the_recorder_buffer_and_only_one_is_out_at_a_time() {
        let mut r = Recorder::new(2, 10, paid()).unwrap();
        assert!(!r.is_recording());
        let t = r.take().unwrap();
        assert_eq!(t.data.capacity(), 20);
        assert!(t.data.is_empty());
        assert_eq!(t.channels, 2);
        assert!(r.data.is_none());
        assert!(r.is_recording());
        assert_eq!(r.take().unwrap_err(), RecordError::Busy);
        // A refused take changed nothing: the live one still owns the
        // buffer and still has somewhere to hand it back.
        assert!(r.is_recording());
        assert!(r.finished.is_some());
        drop(t);
        assert!(!r.is_recording());
        assert!(r.take().is_ok());
    }

    #[test]
    fn a_dropped_take_hands_the_buffer_back_without_losing_capacity() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.take().unwrap();
        t.data.extend([1, 2, 3]);
        drop(t);
        r.collect();
        let data = r.data.as_ref().unwrap();
        assert_eq!(data.as_slice(), [1, 2, 3]);
        assert_eq!(data.capacity(), 10);
        assert!(r.finished.is_none());
    }

    #[test]
    fn a_new_take_starts_over() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.take().unwrap();
        t.data.extend([1, 2, 3]);
        drop(t);
        let t = r.take().unwrap();
        assert!(t.data.is_empty());
        assert_eq!(t.data.capacity(), 10);
    }

    #[test]
    fn a_take_outliving_its_recorder_drops_quietly() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let t = r.take().unwrap();
        drop(r);
        drop(t);
    }

    #[test]
    fn a_take_can_go_to_another_thread() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let t = r.take().unwrap();
        std::thread::spawn(move || drop(t)).join().unwrap();
        assert!(r.take().is_ok());
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
