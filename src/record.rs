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
//! error path, no clock. A take is finished when its buffer fills or when
//! it is dropped, and finishing hands the buffer back to the recorder over
//! a one-slot ring. Nothing travels the other way.
//!
//! ```text
//! control side                                audio side
//! Recorder  ── start_take() moves the buffer ──▶   Take
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
    /// There is nothing to save: no take has been recorded since the
    /// recorder was made or last saved.
    #[error("nothing has been recorded")]
    Nothing,
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
    /// collected. While it is, [`Recorder::start_take`] is refused.
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
    pub fn start_take(&mut self) -> Result<Take, RecordError> {
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

    /// The last take's audio, lent to the engine to store. Refused while a
    /// take is out, or when nothing has been recorded.
    ///
    /// The buffer leaves the recorder for the length of the save and comes
    /// back through [`Recorder::restore`]; the engine holds the recorder
    /// exclusively between the two, so no take can be asked for meanwhile.
    /// Nothing is copied: the buffer is wrapped as it stands.
    pub(crate) fn recorded(&mut self) -> Result<AudioBuffer, RecordError> {
        if self.is_recording() {
            return Err(RecordError::Busy);
        }
        let data = self
            .data
            .as_mut()
            .expect("not recording means the buffer is here");
        if data.is_empty() {
            return Err(RecordError::Nothing);
        }
        // `mem::take` leaves an empty vector that allocates nothing.
        let data = std::mem::take(data);
        Ok(AudioBuffer::new(self.channels, data).expect("a take writes whole frames"))
    }

    /// Takes the buffer back after a save. If `saved`, the recording is
    /// cleared so it is stored once and the next save finds nothing; if
    /// not, it is kept so the save can be tried again. The capacity is the
    /// recorder's either way.
    pub(crate) fn restore(&mut self, audio: AudioBuffer, saved: bool) {
        let mut data = audio.into_samples();
        if saved {
            data.clear();
        }
        self.data = Some(data);
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
/// Fed interleaved `f32` frames with [`Take::write`]. Finishes when the
/// buffer is full or when it is dropped, and either way hands the buffer
/// back to the recorder. Full is the take doing exactly what it was asked;
/// dropping the stream that holds it is how a live take ends early.
pub struct Take {
    /// The recorder's buffer. The length is how much has been written and
    /// the capacity is the recorder's size. Never grows.
    data: Vec<Pcm>,
    /// Channels per frame, one or two.
    channels: u16,
    /// Quantiser state, so every sample is dithered as it lands and the
    /// buffer is in the project format from the first frame.
    dither: Dither,
    /// The one-slot ring back to the recorder. `None` once the buffer has
    /// been handed over.
    finished: Option<Producer<Vec<Pcm>>>,
}

impl Take {
    /// Writes interleaved `f32` frames, as many as fit.
    ///
    /// Each sample is quantised to the project format as it lands. When
    /// the last frame that fits has landed the take finishes and hands
    /// its buffer back; frames after that, in this call or any later one,
    /// are dropped. Only whole frames are taken: a slice that is not a
    /// whole number of frames is a caller bug, and its tail is ignored.
    /// Nothing here allocates or can fail.
    pub fn write(&mut self, frames: &[f32]) {
        if self.finished.is_none() {
            return;
        }
        let channels = usize::from(self.channels);
        debug_assert!(
            frames.len().is_multiple_of(channels),
            "{} samples is not a whole number of {channels}-channel frames",
            frames.len()
        );
        let whole = frames.len() - frames.len() % channels;
        // The capacity and the length are both whole frames, so the room
        // is too, and pushing within it never reallocates.
        let room = self.data.capacity() - self.data.len();
        for &sample in &frames[..whole.min(room)] {
            self.data.push(self.dither.quantise(sample));
        }
        if self.data.len() == self.data.capacity() {
            self.finish();
        }
    }

    /// Whether the take has handed its buffer back.
    pub fn is_finished(&self) -> bool {
        self.finished.is_none()
    }

    /// Hands the buffer back, once. If the recorder is gone the push finds
    /// no receiver and the buffer is freed here instead, wherever here is;
    /// the roadmap lists the free on the audio thread with the renderer's.
    fn finish(&mut self) {
        if let Some(mut finished) = self.finished.take() {
            // `mem::take` leaves an empty vector that allocates nothing.
            let _ = finished.push(std::mem::take(&mut self.data));
        }
    }
}

impl Drop for Take {
    fn drop(&mut self) {
        self.finish();
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
        let t = r.start_take().unwrap();
        assert_eq!(t.data.capacity(), 20);
        assert!(t.data.is_empty());
        assert_eq!(t.channels, 2);
        assert!(r.data.is_none());
        assert!(r.is_recording());
        assert_eq!(r.start_take().unwrap_err(), RecordError::Busy);
        // A refused take changed nothing: the live one still owns the
        // buffer and still has somewhere to hand it back.
        assert!(r.is_recording());
        assert!(r.finished.is_some());
        drop(t);
        assert!(!r.is_recording());
        assert!(r.start_take().is_ok());
    }

    #[test]
    fn a_dropped_take_hands_the_buffer_back_without_losing_capacity() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.start_take().unwrap();
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
        let mut t = r.start_take().unwrap();
        t.data.extend([1, 2, 3]);
        drop(t);
        let t = r.start_take().unwrap();
        assert!(t.data.is_empty());
        assert_eq!(t.data.capacity(), 10);
    }

    /// The buffer the recorder has after its take has come back.
    fn recorded(r: &mut Recorder) -> &[Pcm] {
        r.collect();
        r.data.as_ref().unwrap()
    }

    /// What the crate's quantiser makes of `samples`, for comparing a take
    /// against the same path a whole buffer goes through.
    fn quantised(channels: u16, samples: &[f32]) -> Vec<Pcm> {
        AudioBuffer::from_f32(channels, samples)
            .unwrap()
            .samples()
            .to_vec()
    }

    #[test]
    fn write_quantises_frames_as_from_f32_does() {
        let input = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25, 2.0, -2.0];
        let mut r = Recorder::new(2, 4, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        t.write(&input[..4]);
        assert!(!t.is_finished());
        t.write(&input[4..]);
        assert!(t.is_finished());
        drop(t);
        assert_eq!(recorded(&mut r), quantised(2, &input));
    }

    #[test]
    fn a_full_take_finishes_itself_and_drops_the_rest() {
        let mut r = Recorder::new(1, 3, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        t.write(&[0.1, 0.2, 0.3, 0.4, 0.5]);
        assert!(t.is_finished());
        // The buffer is already back; the take has nothing more to say.
        assert!(!r.is_recording());
        t.write(&[0.9]);
        drop(t);
        assert_eq!(recorded(&mut r), quantised(1, &[0.1, 0.2, 0.3]));
    }

    #[test]
    fn write_never_grows_the_buffer() {
        let mut r = Recorder::new(2, 4, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        let before = t.data.as_ptr();
        t.write(&[0.0; 6]);
        assert_eq!(t.data.as_ptr(), before);
        assert_eq!(t.data.capacity(), 8);
        t.write(&[0.0; 6]);
        drop(t);
        let data = recorded(&mut r);
        assert_eq!(data.len(), 8);
        assert_eq!(data.as_ptr(), before);
    }

    #[test]
    fn write_takes_whole_frames_only() {
        let mut r = Recorder::new(2, 4, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        // Release builds ignore the ragged tail; debug builds assert on it.
        if cfg!(debug_assertions) {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.write(&[0.0; 3])))
                    .is_err()
            );
        } else {
            t.write(&[0.0; 3]);
            assert_eq!(t.data.len(), 2);
        }
    }

    #[test]
    fn a_take_ended_early_keeps_what_was_written() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        t.write(&[0.5, -0.5]);
        drop(t);
        assert_eq!(recorded(&mut r), quantised(1, &[0.5, -0.5]));
    }

    #[test]
    fn recorded_lends_the_buffer_and_restore_returns_it_without_a_copy() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        t.write(&[0.5, -0.5]);
        drop(t);
        let before = recorded(&mut r).as_ptr();

        let audio = r.recorded().unwrap();
        assert_eq!(audio.samples().as_ptr(), before);
        assert_eq!(audio.samples(), quantised(1, &[0.5, -0.5]));
        r.restore(audio, false);
        let data = r.data.as_ref().unwrap();
        assert_eq!(data.as_ptr(), before);
        assert_eq!(data.len(), 2);
        assert_eq!(data.capacity(), 10);

        let audio = r.recorded().unwrap();
        r.restore(audio, true);
        let data = r.data.as_ref().unwrap();
        assert_eq!(data.as_ptr(), before);
        assert!(data.is_empty());
        assert_eq!(data.capacity(), 10);
        assert_eq!(r.recorded().unwrap_err(), RecordError::Nothing);
    }

    #[test]
    fn recorded_is_refused_while_a_take_is_out_or_nothing_was_written() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        assert_eq!(r.recorded().unwrap_err(), RecordError::Nothing);
        let t = r.start_take().unwrap();
        assert_eq!(r.recorded().unwrap_err(), RecordError::Busy);
        drop(t);
        assert_eq!(r.recorded().unwrap_err(), RecordError::Nothing);
    }

    #[test]
    fn a_take_outliving_its_recorder_drops_quietly() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let t = r.start_take().unwrap();
        drop(r);
        drop(t);
    }

    #[test]
    fn a_take_can_be_written_on_another_thread() {
        let mut r = Recorder::new(1, 10, paid()).unwrap();
        let mut t = r.start_take().unwrap();
        std::thread::spawn(move || t.write(&[0.5; 4]))
            .join()
            .unwrap();
        assert_eq!(recorded(&mut r), quantised(1, &[0.5; 4]));
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
