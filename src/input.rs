//! The control side of an input: the handle you hold, the sources on it
//! you monitor, and the feeder a device callback fills.
//!
//! An input is the ability to put audio into the renderer; whether it is
//! heard is a separate switch, monitoring, which is the input mixer's
//! mute. `Engine::input` returns two things. The [`Input`] is the handle:
//! it names the input's sources, each an [`InputSource`] with `monitor`
//! and `unmonitor` on it, and holding it is what keeps the input in the
//! renderer; drop it and the input is removed. The [`InputFeed`] is the
//! other end: it moves into the capture callback and has one method,
//! `feed`, which takes the device's frames.
//!
//! An input is added in the shape of its device: one layout saying what
//! each channel of the device's interleaved frames means, which are
//! stereo pairs and which are mono. Each mono or stereo entry is a source
//! of its own, with its own sound in the renderer, its own ring and its
//! own slot in the input mixer, monitored on its own. The split from the
//! device's frames into the sources happens in [`InputFeed::feed`], where
//! the interleaved frame already is; the render side,
//! [`InputReader`](crate::dsp::InputReader), sees only mono and stereo
//! sources, the same as it sees samples.
//!
//! The feeder is the only piece here that runs on a thread with a hard
//! deadline, and the only one that holds no [`RendererHandle`]: `feed`
//! is a fixed loop over the period into rings that already exist, no lock,
//! no allocation. The rings are allocated at `open`, on the control side,
//! and are not on the memory ledger: they scale with the device's period
//! and the layout, both bounded, not with the audio held, and their worst
//! case is [`MAX_RING_BYTES`], fixed overhead.
//!
//! Both ends hold a ring; it is freed when the second goes. Drop the
//! `Input` first and the `InputFeed` second, so the render side lets go
//! of its end at the next render and the free happens on the caller's
//! thread. Dropped the other way round, the readers finish on their own
//! and the input mixer frees the rings on the audio thread.

use std::mem::size_of;

use rtrb::Producer;

use crate::config::{MAX_INPUT_CHANNELS, MAX_PERIOD_FRAMES, Pcm};
use crate::dsp::{InputReader, SoundId};
use crate::engine::EngineError;
use crate::renderer_handle::RendererHandle;

/// How many of the capture device's periods a source's ring holds: enough
/// that the two clocks can disagree by [`MAX_LAG_PERIODS`] of output and
/// a capture period can land on top before a frame is dropped. The ring
/// is whole frames of the source's width, so a frame never wraps.
///
/// [`MAX_LAG_PERIODS`]: crate::dsp::input::MAX_LAG_PERIODS
const RING_PERIODS: usize = 8;

/// The most memory every input ring together can take: every source of
/// every input at the widest layout and the longest period. The rings
/// are fixed overhead outside the memory ledger because this number
/// exists; it is a few megabytes, and a bigger one is a change here.
pub const MAX_RING_BYTES: usize =
    MAX_INPUT_CHANNELS * MAX_PERIOD_FRAMES * RING_PERIODS * size_of::<Pcm>();
const _: () = assert!(
    MAX_RING_BYTES <= 4 << 20,
    "input rings are meant to be a few megabytes of fixed overhead"
);

/// What a run of a device's channels means: one channel heard on both
/// sides, or two heard left and right. Interleaved stereo is left then
/// right everywhere, so a pair is always the two channels in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    Mono,
    Stereo,
}

impl Channels {
    /// How many channels the source takes.
    pub fn count(self) -> usize {
        match self {
            Channels::Mono => 1,
            Channels::Stereo => 2,
        }
    }
}

/// How many channels wide `layout` is: the width of the device frames an
/// input laid out that way is fed.
pub fn width(layout: &[Channels]) -> usize {
    layout.iter().map(|c| c.count()).sum()
}

/// One source of an input, a mono channel or a stereo pair of the device,
/// as a sound in the renderer. From [`Input::sources`], in layout order.
/// Not heard until monitored.
#[derive(Debug)]
pub struct InputSource {
    id: SoundId,
    renderer: RendererHandle,
}

impl InputSource {
    /// The id the renderer knows this source's sound by.
    #[cfg(test)]
    pub(crate) fn id(&self) -> SoundId {
        self.id
    }

    /// Hears the source through the output from the next period. Refused
    /// as `CommandsFull` if the ring has no room.
    pub fn monitor(&self) -> Result<(), EngineError> {
        self.renderer.monitor(self.id, true)
    }

    /// Stops hearing the source. It stays an input and keeps up with the
    /// device; `monitor` hears it again. Refused as `CommandsFull` if the
    /// ring has no room.
    pub fn unmonitor(&self) -> Result<(), EngineError> {
        self.renderer.monitor(self.id, false)
    }
}

/// An input the renderer holds: the handle from `Engine::input`. Its
/// sources are what you monitor; dropping it removes the input.
#[derive(Debug)]
pub struct Input {
    /// One per entry of the layout, in order.
    sources: Vec<InputSource>,
    renderer: RendererHandle,
}

impl Input {
    /// The input whose sources are the sounds `sounds`, in layout order,
    /// on `renderer`.
    pub(crate) fn new(sounds: &[SoundId], renderer: RendererHandle) -> Self {
        let sources = sounds
            .iter()
            .map(|&id| InputSource {
                id,
                renderer: renderer.clone(),
            })
            .collect();
        Self { sources, renderer }
    }

    /// The input's sources, one per entry of the layout it was added
    /// with, in order.
    pub fn sources(&self) -> &[InputSource] {
        &self.sources
    }
}

impl Drop for Input {
    /// Removes the input: tells the renderer to drop every source's reader
    /// at its next render and gives the sources' slots back. Best effort:
    /// a drop cannot fail, so if the ring has no room for the removes the
    /// readers stay until the `InputFeed` is dropped, when they finish on
    /// their own, and their slots stay spoken for.
    fn drop(&mut self) {
        let sounds: Vec<SoundId> = self.sources.iter().map(|s| s.id).collect();
        let _ = self.renderer.remove_input(&sounds);
    }
}

/// The writing end of one source's ring, and where in a device frame the
/// source's channels are.
#[derive(Debug)]
struct SourceWriter {
    frames: Producer<Pcm>,
    /// The source's first channel in the device frame.
    first: usize,
    /// How many channels from there: 1 or 2.
    channels: usize,
}

/// The capture side of an input: takes periods of device frames from
/// whoever owns the capture thread and splits them into its sources.
///
/// From `Engine::input`, moved into a device's capture callback. Drop the
/// [`Input`] first, then this with the stream that drives it, then the
/// engine.
#[derive(Debug)]
pub struct InputFeed {
    /// One per source of the layout, in order.
    sources: Vec<SourceWriter>,
    /// Samples per device frame: the layout's width.
    width: usize,
}

impl InputFeed {
    /// Both ends of a new input laid out as `layout`, fed `period` frames
    /// at a time, at most `MAX_PERIOD_FRAMES`. The rings are sized from
    /// the period and allocated here, on the control side. The feed goes
    /// to the capture callback; each [`InputReader`], one per entry of
    /// `layout` in order, crosses to the render side on the command that
    /// adds its source.
    pub(crate) fn open(layout: &[Channels], period: usize) -> (InputFeed, Vec<InputReader>) {
        debug_assert!(
            (1..=MAX_PERIOD_FRAMES).contains(&period),
            "the engine checks the period"
        );
        Self::open_with(layout, period * RING_PERIODS)
    }

    /// [`open`](Self::open) with rings of `frames` frames.
    fn open_with(layout: &[Channels], frames: usize) -> (InputFeed, Vec<InputReader>) {
        let mut sources = Vec::with_capacity(layout.len());
        let mut readers = Vec::with_capacity(layout.len());
        let mut first = 0;
        for &channels in layout {
            // This allocation is seen and deliberately not on the ledger.
            // `rtrb` allocates the ring itself, so it could only be counted
            // beside the buffer, not as one; and it does not scale with
            // the audio held, only with the period and the layout, both
            // bounded, so its worst case is `MAX_RING_BYTES` and it is
            // taken as part of the engine's fixed overhead.
            let (writer, reader) = rtrb::RingBuffer::new(frames * channels.count());
            sources.push(SourceWriter {
                frames: writer,
                first,
                channels: channels.count(),
            });
            readers.push(InputReader::new(reader, channels.count() as u16));
            first += channels.count();
        }
        let feed = InputFeed {
            sources,
            width: first,
        };
        (feed, readers)
    }

    /// How many samples make a device frame: the layout's width.
    pub fn channels(&self) -> usize {
        self.width
    }

    /// Feeds one period of interleaved device frames,
    /// [`channels`](Self::channels) samples each, as the device delivered
    /// them, splitting each frame into the sources' rings.
    ///
    /// Never blocks. Frames a source's ring has no room for are dropped
    /// from that source, whole frames at a time, so a ring never holds a
    /// partial frame; the render side hears a gap, not a channel swap. A
    /// trailing partial device frame is dropped for the same reason. Once
    /// the input is removed every frame is dropped.
    pub fn feed(&mut self, frames: &[Pcm]) {
        debug_assert_eq!(frames.len() % self.width, 0, "a period is whole frames");
        let frames = frames.chunks_exact(self.width);
        for source in &mut self.sources {
            let room = source.frames.slots() / source.channels;
            let taken = frames.len().min(room);
            let samples = frames.clone().take(taken).flat_map(|frame| {
                frame[source.first..source.first + source.channels]
                    .iter()
                    .copied()
            });
            if let Ok(chunk) = source.frames.write_chunk_uninit(taken * source.channels) {
                chunk.fill_from_iter(samples);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Channels::{Mono, Stereo};
    use super::*;
    use crate::dsp::Source;

    /// A feed laid out as `layout` over rings of `frames` frames, and its
    /// readers.
    fn feed(layout: &[Channels], frames: usize) -> (InputFeed, Vec<InputReader>) {
        InputFeed::open_with(layout, frames)
    }

    #[test]
    fn a_layout_is_as_wide_as_its_sources() {
        assert_eq!(width(&[]), 0);
        assert_eq!(width(&[Mono]), 1);
        assert_eq!(width(&[Stereo, Mono, Mono]), 4);
    }

    #[test]
    fn a_device_frame_is_split_into_its_sources() {
        // A pair on channels 0 and 1, a microphone on 2, another on 3.
        let (mut f, mut readers) = feed(&[Stereo, Mono, Mono], 8);
        assert_eq!(f.channels(), 4);
        assert_eq!(readers.len(), 3);
        f.feed(&[0.1, 0.2, 0.4, 0.8, 0.11, 0.22, 0.44, 0.88]);

        let mut out = vec![0.0; 4];
        readers[0].add(&mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.11, 0.22], "the pair, left and right");
        let mut out = vec![0.0; 4];
        readers[1].add(&mut out);
        assert_eq!(out, vec![0.4, 0.4, 0.44, 0.44], "the first microphone");
        let mut out = vec![0.0; 4];
        readers[2].add(&mut out);
        assert_eq!(out, vec![0.8, 0.8, 0.88, 0.88], "the second");
    }

    #[test]
    fn a_full_ring_drops_whole_frames_from_that_source_only() {
        let (mut f, mut readers) = feed(&[Stereo, Mono], 1);
        // One frame fits in each ring; the second frame finds both full.
        f.feed(&[0.1, 0.5, 0.9, 0.2, 0.6, 0.8]);
        let mut out = vec![0.0; 4];
        readers[0].add(&mut out);
        assert_eq!(out, vec![0.1, 0.5, 0.0, 0.0]);
        let mut out = vec![0.0; 4];
        readers[1].add(&mut out);
        assert_eq!(out, vec![0.9, 0.9, 0.0, 0.0]);
    }

    #[test]
    fn dropping_the_feed_finishes_every_reader() {
        let (f, readers) = feed(&[Stereo, Mono], 8);
        assert!(readers.iter().all(|r| !r.finished()));
        drop(f);
        assert!(readers.iter().all(|r| r.finished()));
    }

    #[test]
    fn open_sizes_each_ring_from_the_period_in_whole_frames_of_its_width() {
        let (mut f, mut readers) = InputFeed::open(&[Mono, Stereo], 4);
        assert_eq!(readers[0].capacity(), 4 * RING_PERIODS);
        assert_eq!(readers[1].capacity(), 4 * RING_PERIODS * 2);
        assert_eq!(f.channels(), 3);
        f.feed(&[0.9, 0.1, 0.5]);
        let mut out = vec![0.0; 2];
        readers[1].add(&mut out);
        assert_eq!(out, vec![0.1, 0.5]);
    }
}

#[cfg(test)]
mod worst_case {
    use super::*;

    #[test]
    fn the_widest_input_at_the_longest_period_takes_exactly_the_worst_case() {
        let widest = [Channels::Stereo; MAX_INPUT_CHANNELS / 2];
        let (_f, readers) = InputFeed::open(&widest, MAX_PERIOD_FRAMES);
        let bytes: usize = readers
            .iter()
            .map(|r| r.capacity() * size_of::<Pcm>())
            .sum();
        assert_eq!(bytes, MAX_RING_BYTES);
    }
}
