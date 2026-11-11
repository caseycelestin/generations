//! The render side of an input: one source's ring, read a period at a
//! time and heard.
//!
//! An input is added on the control side, in [`crate::control::input`], as one
//! ring per mono or stereo source; this is the other end of one of those
//! rings. An [`InputReader`] is a [`Source`]: each period it gives one
//! period's worth of frames off its ring as stereo frames, as a
//! [`SampleReader`](crate::render::dsp::SampleReader) gives a sample's,
//! mono on both sides, stereo left and right. It knows nothing of the
//! device, the layout or the other sources: frames of one or two samples
//! arrive, and that is all. Nothing here blocks or allocates.
//!
//! The capture and output callbacks run on their own clocks, so the ring
//! drifts. Empty is a gap: no frames are given and the period goes on.
//! Fuller than [`MAX_LAG_PERIODS`] of output is latency the ear notices,
//! so the surplus is skipped and the reader catches up with a click. Both ends
//! hold the ring; the reader is finished once the writing end is dropped,
//! and whoever holds it may then let it go.

use rtrb::Consumer;

use crate::audio::ChannelLayout;
use crate::config::{MAX_CHANNELS, Pcm};
use crate::render::dsp::Source;
use crate::render::dsp::math::{Frame, sides_for};

/// How far an input may fall behind the output, in output periods,
/// before its reader skips ahead.
pub const MAX_LAG_PERIODS: usize = 2;

/// The render side of one source of an input: reads its ring and is
/// heard.
#[derive(Debug)]
pub struct InputReader {
    /// The reading end of the source's ring.
    frames: Consumer<Pcm>,
    /// Mono or stereo frames on the ring.
    layout: ChannelLayout,
}

impl InputReader {
    /// A reader of `frames`, frames of `layout`. The ring's capacity must
    /// be whole frames, so a frame never wraps.
    pub(crate) fn new(frames: Consumer<Pcm>, layout: ChannelLayout) -> Self {
        debug_assert_eq!(
            frames.buffer().capacity() % layout.count(),
            0,
            "the input ring is not whole frames"
        );
        Self { frames, layout }
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
        let width = self.layout.count();
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
    /// The next `frames` frames off the ring as stereo, mono on both
    /// sides, stereo left and right; fewer, down to none, where the ring
    /// runs out. Skips ahead first when the ring holds more than
    /// [`MAX_LAG_PERIODS`] periods. The frames given leave the ring when
    /// the iterator is dropped; any not taken stay for the next period.
    fn frames(&mut self, frames: usize) -> impl Iterator<Item = Frame> {
        let (width, sides) = (self.layout.count(), sides_for(self.layout));
        let mut samples = self.take(frames).into_iter().flatten();
        // Whole frames on the ring: the writer pushes frames and the
        // capacity is a multiple of one, so a frame never runs short.
        std::iter::from_fn(move || {
            let mut frame = [0.0; MAX_CHANNELS as usize];
            for sample in frame.iter_mut().take(width) {
                *sample = samples.next()?;
            }
            Some(sides.map(|side| frame[side]))
        })
    }

    /// Finished once the `InputFeed` writing its ring is dropped.
    fn finished(&self) -> bool {
        self.frames.is_abandoned()
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;
    use crate::audio::ChannelLayout::{Mono, Stereo};
    use crate::testing::hear;

    /// A reader of `layout` over a ring of `frames` frames, and the
    /// writing end to feed it whole frames through.
    fn reader(layout: ChannelLayout, frames: usize) -> (rtrb::Producer<Pcm>, InputReader) {
        let (writer, reader) = RingBuffer::new(frames * layout.count());
        (writer, InputReader::new(reader, layout))
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
        let (mut w, mut r) = reader(Mono, 8);
        feed(&mut w, &[0.5, 0.6]);
        let mut out = vec![0.0; 4];
        hear(&mut r, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.6, 0.6]);
    }

    #[test]
    fn stereo_adds_into_what_is_there() {
        let (mut w, mut r) = reader(Stereo, 8);
        feed(&mut w, &TWO);
        let mut out = vec![1.0; 4];
        hear(&mut r, &mut out);
        assert_eq!(out, vec![1.1, 1.5, 1.2, 1.6]);
    }

    #[test]
    fn an_empty_ring_is_a_gap() {
        let (mut w, mut r) = reader(Stereo, 8);
        feed(&mut w, &TWO);
        let mut out = vec![0.0; 8];
        hear(&mut r, &mut out);
        assert_eq!(out, vec![0.1, 0.5, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn too_much_lag_is_skipped() {
        let (mut w, mut r) = reader(Mono, 16);
        // Twelve frames queued against a one-frame period: all but two are
        // skipped, the oldest first.
        let frames: Vec<Pcm> = (0..12).map(|n| n as Pcm).collect();
        feed(&mut w, &frames);
        let mut out = vec![0.0; 2];
        hear(&mut r, &mut out);
        assert_eq!(out, vec![10.0, 10.0]);
        hear(&mut r, &mut out);
        assert_eq!(out, vec![21.0, 21.0]);
    }

    #[test]
    fn dropping_the_writing_end_finishes_the_reader() {
        let (w, r) = reader(Mono, 8);
        assert!(!r.finished());
        drop(w);
        assert!(r.finished());
    }
}
