//! The input: frames from a capture device, heard through the output.
//!
//! An input is the ability to put audio into the renderer; whether it is
//! heard is a separate switch, monitoring, which is the input mixer's
//! mute. Two ends of one ring. The [`Input`] is the handle the engine
//! mints and the caller moves into the capture callback, where it is
//! given each period the device recorded. The [`InputReader`] is the
//! other end: a [`Source`] the renderer's input mixer holds, which each
//! period takes one period's worth of frames off the ring and, when
//! monitored, adds two chosen channels of them into the output. Nothing
//! on either end blocks or allocates, so both audio threads can call it
//! with a hard deadline.
//!
//! The ring is made when the input is added and crosses to the render
//! side on the command that adds it, so each input is its own ring and
//! there is nothing to park when one is removed. Both ends hold the
//! ring; it is freed when the second goes. Remove the input first and
//! drop the `Input` second, so the free happens on the caller's thread.
//! Dropping the `Input` first finishes the reader, which frees the ring
//! on the audio thread when the mixer lets the slot go.
//!
//! The capture and output callbacks run on their own clocks, so the ring
//! drifts. Empty is a gap: nothing is added and the period goes on. Fuller
//! than [`MAX_LAG_PERIODS`] of output is latency the ear notices, so the
//! surplus is skipped and the reader catches up with a click.

use rtrb::{Consumer, Producer};

use crate::config::{INPUT_CHANNELS, OUTPUT_CHANNELS, Pcm};
use crate::dsp::Source;
use crate::dsp::command::SoundId;
use crate::dsp::math::{Sides, add, stereo};

/// How far the input may fall behind the output, in output periods,
/// before the reader skips ahead.
pub const MAX_LAG_PERIODS: usize = 2;

/// The sides for `channels` of the device's `INPUT_CHANNELS`, in output
/// order: one channel on both sides, or a left and a right. `None` if it
/// is not one or two channels the device has.
pub(crate) fn sides(channels: &[usize]) -> Option<Sides> {
    let sides = match channels {
        [c] => [*c, *c],
        [l, r] => [*l, *r],
        _ => return None,
    };
    sides.iter().all(|&c| c < INPUT_CHANNELS).then_some(sides)
}

/// The capture side of an input: takes periods of frames from whoever
/// owns the capture thread.
///
/// From `Engine::input`, moved into a device's capture callback.
/// `Engine::remove_input` it, then drop it with the stream that drives
/// it, before the engine.
#[derive(Debug)]
pub struct Input {
    /// The writing end of the ring.
    frames: Producer<Pcm>,
    /// The sound this input is in the input mixer as.
    sound: SoundId,
}

impl Input {
    pub(crate) fn new(frames: Producer<Pcm>, sound: SoundId) -> Self {
        Self { frames, sound }
    }

    /// The sound this input is in the input mixer as: what
    /// `Engine::monitor`, `unmonitor` and `remove_input` take.
    pub fn sound(&self) -> SoundId {
        self.sound
    }

    /// Feeds one period of interleaved frames, `INPUT_CHANNELS` wide, as
    /// the capture device delivered them.
    ///
    /// Never blocks. Frames the ring has no room for are dropped, whole
    /// frames at a time, so the ring never holds a partial frame; the
    /// render side hears a gap, not a channel swap. Once the input is
    /// removed every frame is dropped.
    pub fn feed(&mut self, frames: &[Pcm]) {
        debug_assert_eq!(frames.len() % INPUT_CHANNELS, 0, "a period is whole frames");
        let room = self.frames.slots() - self.frames.slots() % INPUT_CHANNELS;
        let taken = frames.len().min(room);
        if let Ok(chunk) = self.frames.write_chunk_uninit(taken) {
            chunk.fill_from_iter(frames[..taken].iter().copied());
        }
    }
}

/// The render side of an input: reads the ring and is heard.
#[derive(Debug)]
pub struct InputReader {
    /// The reading end of the ring.
    frames: Consumer<Pcm>,
    /// Which capture channel lands on each output side.
    sides: Sides,
}

impl InputReader {
    /// A reader of `frames`, hearing `sides`. The ring's capacity must be
    /// whole frames, so a frame never wraps.
    pub fn new(frames: Consumer<Pcm>, sides: Sides) -> Self {
        debug_assert_eq!(
            frames.buffer().capacity() % INPUT_CHANNELS,
            0,
            "the input ring is not whole frames"
        );
        Self { frames, sides }
    }
}

impl InputReader {
    /// The next period's worth of frames off the ring, as a chunk to read
    /// and commit, after skipping ahead if more than [`MAX_LAG_PERIODS`]
    /// periods are queued. `None` when the ring is empty.
    fn take(&mut self, frames: usize) -> Option<rtrb::chunks::ReadChunk<'_, Pcm>> {
        let queued = self.frames.slots() / INPUT_CHANNELS;
        let lag = queued.saturating_sub(frames * MAX_LAG_PERIODS);
        if lag > 0
            && let Ok(skipped) = self.frames.read_chunk(lag * INPUT_CHANNELS)
        {
            skipped.commit_all();
        }
        let take = frames.min(queued - lag);
        self.frames.read_chunk(take * INPUT_CHANNELS).ok()
    }
}

impl Source for InputReader {
    /// Takes one period of input off the ring and adds it to `out`. Adds
    /// nothing where the ring runs out; skips ahead when it holds more
    /// than [`MAX_LAG_PERIODS`] periods.
    fn add(&mut self, out: &mut [Pcm]) {
        let sides = self.sides;
        let Some(chunk) = self.take(out.len() / OUTPUT_CHANNELS) else {
            return;
        };
        // Whole frames in, whole frames per slice: the writer pushes
        // frames and the capacity is a multiple of one.
        let (first, second) = chunk.as_slices();
        add(
            out,
            stereo(first, INPUT_CHANNELS, sides).chain(stereo(second, INPUT_CHANNELS, sides)),
        );
        chunk.commit_all();
    }

    /// Takes one period of input off the ring and drops it, so an input
    /// that is not monitored keeps up with the device.
    fn skip(&mut self, frames: usize) {
        if let Some(chunk) = self.take(frames) {
            chunk.commit_all();
        }
    }

    /// Finished once the `Input` feeding it is dropped.
    fn finished(&self) -> bool {
        self.frames.is_abandoned()
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    fn input(frames: usize, sides: Sides) -> (Input, InputReader) {
        let (writer, reader) = RingBuffer::new(frames * INPUT_CHANNELS);
        (
            Input::new(writer, SoundId(0)),
            InputReader::new(reader, sides),
        )
    }

    /// Two frames: channel 0 is 0.1 then 0.2, channel 1 is 0.5 then 0.6.
    const TWO: [Pcm; 4] = [0.1, 0.5, 0.2, 0.6];

    #[test]
    fn mono_is_heard_on_both_sides() {
        let (mut i, mut r) = input(8, [1, 1]);
        i.feed(&TWO);
        let mut out = vec![0.0; 4];
        r.add(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.6, 0.6]);
    }

    #[test]
    fn stereo_adds_into_what_is_there() {
        let (mut i, mut r) = input(8, [1, 0]);
        i.feed(&TWO);
        let mut out = vec![1.0; 4];
        r.add(&mut out);
        assert_eq!(out, vec![1.5, 1.1, 1.6, 1.2]);
    }

    #[test]
    fn an_empty_ring_is_a_gap() {
        let (mut i, mut r) = input(8, [0, 0]);
        i.feed(&TWO);
        let mut out = vec![0.0; 8];
        r.add(&mut out);
        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn too_much_lag_is_skipped() {
        let (mut i, mut r) = input(16, [0, 0]);
        // Twelve frames queued against a one-frame period: all but two are
        // skipped, the oldest first.
        let frames: Vec<Pcm> = (0..12).flat_map(|n| [n as Pcm, 0.0]).collect();
        i.feed(&frames);
        let mut out = vec![0.0; 2];
        r.add(&mut out);
        assert_eq!(out, vec![10.0, 10.0]);
        r.add(&mut out);
        assert_eq!(out, vec![21.0, 21.0]);
    }

    #[test]
    fn a_skip_drops_a_period_and_the_next_add_hears_what_follows() {
        let (mut i, mut r) = input(8, [0, 0]);
        i.feed(&TWO);
        r.skip(1);
        let mut out = vec![0.0; 2];
        r.add(&mut out);
        assert_eq!(out, vec![0.2, 0.2]);
    }

    #[test]
    fn a_full_ring_drops_whole_frames() {
        let (mut i, mut r) = input(1, [0, 0]);
        i.feed(&TWO);
        let mut out = vec![0.0; 4];
        r.add(&mut out);
        assert_eq!(out, vec![0.1, 0.1, 0.0, 0.0]);
    }

    #[test]
    fn dropping_the_input_finishes_the_reader() {
        let (i, r) = input(8, [0, 0]);
        assert!(!r.finished());
        drop(i);
        assert!(r.finished());
    }

    #[test]
    fn sides_are_checked_against_the_input_width() {
        assert_eq!(sides(&[0]), Some([0, 0]));
        assert_eq!(sides(&[1, 0]), Some([1, 0]));
        assert_eq!(sides(&[]), None);
        assert_eq!(sides(&[0, 1, 0]), None);
        assert_eq!(sides(&[INPUT_CHANNELS]), None);
    }
}
