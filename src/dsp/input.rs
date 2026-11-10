//! The render side of an input: one source's ring, read a period at a
//! time and heard.
//!
//! An input is added on the control side, in [`crate::input`], as one
//! ring per mono or stereo source; this is the other end of one of those
//! rings. An [`InputReader`] is a [`Source`] the renderer's input mixer
//! holds, which each period takes one period's worth of frames off its
//! ring and, when heard, adds them into the output as a
//! [`Playhead`](crate::dsp::Playhead) adds a sample, mono on both sides,
//! stereo left and right. It knows nothing of the device, the layout or
//! the other sources: frames of one or two samples arrive, and that is
//! all. Nothing here blocks or allocates.
//!
//! The capture and output callbacks run on their own clocks, so the ring
//! drifts. Empty is a gap: nothing is added and the period goes on. Fuller
//! than [`MAX_LAG_PERIODS`] of output is latency the ear notices, so the
//! surplus is skipped and the reader catches up with a click. Both ends
//! hold the ring; the reader is finished once the writing end is dropped,
//! and the mixer then lets its slot go.

use rtrb::Consumer;

use crate::config::{OUTPUT_CHANNELS, Pcm};
use crate::dsp::Source;
use crate::dsp::math::{add, sides_for, stereo};

/// How far an input may fall behind the output, in output periods,
/// before its reader skips ahead.
pub const MAX_LAG_PERIODS: usize = 2;

/// The render side of one source of an input: reads its ring and is
/// heard.
#[derive(Debug)]
pub struct InputReader {
    /// The reading end of the source's ring.
    frames: Consumer<Pcm>,
    /// Samples per frame on the ring: 1 or 2.
    channels: u16,
}

impl InputReader {
    /// A reader of `frames`, `channels` samples per frame. The ring's
    /// capacity must be whole frames, so a frame never wraps.
    pub(crate) fn new(frames: Consumer<Pcm>, channels: u16) -> Self {
        debug_assert!(channels > 0, "a source has a channel");
        debug_assert_eq!(
            frames.buffer().capacity() % usize::from(channels),
            0,
            "the input ring is not whole frames"
        );
        Self { frames, channels }
    }

    /// How many samples the ring holds.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.frames.buffer().capacity()
    }

    /// The next period's worth of frames off the ring, as a chunk to read
    /// and commit, after skipping ahead if more than [`MAX_LAG_PERIODS`]
    /// periods are queued. `None` when the ring is empty.
    fn take(&mut self, frames: usize) -> Option<rtrb::chunks::ReadChunk<'_, Pcm>> {
        let width = usize::from(self.channels);
        let queued = self.frames.slots() / width;
        let lag = queued.saturating_sub(frames * MAX_LAG_PERIODS);
        if lag > 0
            && let Ok(skipped) = self.frames.read_chunk(lag * width)
        {
            skipped.commit_all();
        }
        let take = frames.min(queued - lag);
        self.frames.read_chunk(take * width).ok()
    }
}

impl Source for InputReader {
    /// Takes one period off the ring and adds it to `out`, mono on both
    /// sides, stereo left and right. Adds nothing where the ring runs
    /// out; skips ahead when it holds more than [`MAX_LAG_PERIODS`]
    /// periods.
    fn add(&mut self, out: &mut [Pcm]) {
        let (width, sides) = (usize::from(self.channels), sides_for(self.channels));
        let Some(chunk) = self.take(out.len() / OUTPUT_CHANNELS) else {
            return;
        };
        // Whole frames in, whole frames per slice: the writer pushes
        // frames and the capacity is a multiple of one.
        let (first, second) = chunk.as_slices();
        add(
            out,
            stereo(first, width, sides).chain(stereo(second, width, sides)),
        );
        chunk.commit_all();
    }

    /// Takes one period off the ring and drops it, so a source that is not
    /// heard keeps up with the device.
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

    /// A reader of `channels` channels over a ring of `frames` frames, and
    /// the writing end to feed it whole frames through.
    fn reader(channels: u16, frames: usize) -> (rtrb::Producer<Pcm>, InputReader) {
        let (writer, reader) = RingBuffer::new(frames * usize::from(channels));
        (writer, InputReader::new(reader, channels))
    }

    fn feed(writer: &mut rtrb::Producer<Pcm>, samples: &[Pcm]) {
        for &s in samples {
            writer.push(s).unwrap();
        }
    }

    /// Two stereo frames: left is 0.1 then 0.2, right is 0.5 then 0.6.
    const TWO: [Pcm; 4] = [0.1, 0.5, 0.2, 0.6];

    #[test]
    fn mono_is_heard_on_both_sides() {
        let (mut w, mut r) = reader(1, 8);
        feed(&mut w, &[0.5, 0.6]);
        let mut out = vec![0.0; 4];
        r.add(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.6, 0.6]);
    }

    #[test]
    fn stereo_adds_into_what_is_there() {
        let (mut w, mut r) = reader(2, 8);
        feed(&mut w, &TWO);
        let mut out = vec![1.0; 4];
        r.add(&mut out);
        assert_eq!(out, vec![1.1, 1.5, 1.2, 1.6]);
    }

    #[test]
    fn an_empty_ring_is_a_gap() {
        let (mut w, mut r) = reader(2, 8);
        feed(&mut w, &TWO);
        let mut out = vec![0.0; 8];
        r.add(&mut out);
        assert_eq!(out, vec![0.1, 0.5, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn too_much_lag_is_skipped() {
        let (mut w, mut r) = reader(1, 16);
        // Twelve frames queued against a one-frame period: all but two are
        // skipped, the oldest first.
        let frames: Vec<Pcm> = (0..12).map(|n| n as Pcm).collect();
        feed(&mut w, &frames);
        let mut out = vec![0.0; 2];
        r.add(&mut out);
        assert_eq!(out, vec![10.0, 10.0]);
        r.add(&mut out);
        assert_eq!(out, vec![21.0, 21.0]);
    }

    #[test]
    fn a_skip_drops_a_period_and_the_next_add_hears_what_follows() {
        let (mut w, mut r) = reader(1, 8);
        feed(&mut w, &[0.1, 0.2]);
        r.skip(1);
        let mut out = vec![0.0; 2];
        r.add(&mut out);
        assert_eq!(out, vec![0.2, 0.2]);
    }

    #[test]
    fn dropping_the_writing_end_finishes_the_reader() {
        let (w, r) = reader(1, 8);
        assert!(!r.finished());
        drop(w);
        assert!(r.finished());
    }
}
