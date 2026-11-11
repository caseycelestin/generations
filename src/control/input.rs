//! The control side of an input: the handle you hold, the sounds on it
//! you monitor, and the feeder a device callback fills.
//!
//! An input is the ability to put audio into the renderer; whether it is
//! heard is a separate switch, monitoring, which sets each of its sounds'
//! gain to full or to nothing. `Control::input` returns two things. The [`Input`] is the
//! handle: it names the input's sounds, each an [`InputSound`] with
//! `monitor` and `unmonitor` on it, and holding it is what keeps the input
//! in the renderer; drop it and the input is removed. The [`InputFeed`] is
//! the other end: it moves into the capture callback and has one method,
//! `feed`, which takes the device's frames.
//!
//! An input is added in the shape of its device: an [`InputLayout`]
//! saying what each channel of the device's interleaved frames means,
//! which are stereo pairs and which are mono. Each mono or stereo entry
//! is a sound of its own, with its own ring and its own place in the
//! renderer, monitored on its own. The split from the
//! device's frames into the sources happens in [`InputFeed::feed`], where
//! the interleaved frame already is; the render side,
//! [`InputReader`](crate::render::dsp::InputReader), sees only mono and stereo
//! sources, the same as it sees samples.
//!
//! The feeder is the only piece here that runs on a thread with a hard
//! deadline, and the only one that holds no [`Control`]: `feed`
//! is a fixed loop over the period into rings that already exist, no lock,
//! no allocation. The rings are allocated when the feed is made, on the
//! control side, and are not on the memory ledger: they scale with the
//! device's period and the layout, both bounded, not with the audio held,
//! and their worst case is [`MAX_RING_BYTES`], fixed overhead.
//!
//! Both ends hold a ring; it is freed when the second goes. Drop the
//! `Input` first and the `InputFeed` second, so the render side lets go
//! of its end at the next render and the free happens on the caller's
//! thread. Dropped the other way round, the readers finish on their own
//! and the renderer frees the rings on the audio thread.

use std::mem::size_of;

use rtrb::Producer;

use crate::audio::ChannelLayout;
use crate::config::{MAX_INPUT_CHANNELS, MAX_PERIOD_FRAMES, Pcm};
use crate::control::Control;
use crate::control::ControlError;
use crate::render::command::SoundId;
use crate::render::dsp::InputReader;

/// How many of the capture device's periods a source's ring holds: enough
/// that the two clocks can disagree by [`MAX_LAG_PERIODS`] of output and
/// a capture period can land on top before a frame is dropped. The ring
/// is whole frames of the source's width, so a frame never wraps.
///
/// [`MAX_LAG_PERIODS`]: crate::render::dsp::input_reader::MAX_LAG_PERIODS
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

/// The shape of an input's device: what each channel of its interleaved
/// frames is, in order. Each entry, one [`ChannelLayout`], is a mono
/// channel or a stereo pair and becomes one [`InputSound`]; `[Stereo,
/// Mono]` is a device of three channels, a pair on the first two and a
/// microphone on the third.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputLayout(Vec<ChannelLayout>);

impl InputLayout {
    /// A layout of `entries`, in device channel order.
    pub fn new(entries: impl Into<Vec<ChannelLayout>>) -> Self {
        Self(entries.into())
    }

    /// The entries, in device channel order: one per sound.
    pub fn entries(&self) -> &[ChannelLayout] {
        &self.0
    }

    /// How many channels wide the device frames are: the entries' widths
    /// added up.
    pub fn width(&self) -> usize {
        self.0.iter().map(|c| c.count()).sum()
    }
}

/// One sound of an input, a mono channel or a stereo pair of the device,
/// as a sound in the renderer. From [`Input::sounds`], in layout order.
/// Not heard until monitored.
#[derive(Debug)]
pub struct InputSound {
    id: SoundId,
    control: Control,
}

impl InputSound {
    /// The id the renderer knows this sound by.
    #[cfg(test)]
    pub(crate) fn id(&self) -> SoundId {
        self.id
    }

    /// Hears the sound through the output from the next period. Refused
    /// as `CommandsFull` if the ring has no room.
    pub fn monitor(&self) -> Result<(), ControlError> {
        self.control.set_monitored(self.id, true)
    }

    /// Stops hearing the sound. It stays part of the input and keeps up with the
    /// device; `monitor` hears it again. Refused as `CommandsFull` if the
    /// ring has no room.
    pub fn unmonitor(&self) -> Result<(), ControlError> {
        self.control.set_monitored(self.id, false)
    }
}

/// An input the renderer holds: the handle from `Control::input`. Its
/// sounds are what you monitor; dropping it removes the input.
#[derive(Debug)]
pub struct Input {
    /// One per entry of the layout, in order.
    sounds: Vec<InputSound>,
    control: Control,
}

impl Input {
    /// The input whose sounds are named `ids`, in layout order, on
    /// `control`.
    pub(crate) fn new(ids: &[SoundId], control: Control) -> Self {
        let sounds = ids
            .iter()
            .map(|&id| InputSound {
                id,
                control: control.clone(),
            })
            .collect();
        Self { sounds, control }
    }

    /// The input's sounds, one per entry of the layout it was added
    /// with, in order.
    pub fn sounds(&self) -> &[InputSound] {
        &self.sounds
    }
}

impl Drop for Input {
    /// Removes the input: tells the renderer to drop every sound's reader
    /// at its next render and stops counting them as held. Best effort:
    /// a drop cannot fail, so if the ring has no room for the removes the
    /// readers stay until the `InputFeed` is dropped, when they finish on
    /// their own, and they stay counted as held.
    fn drop(&mut self) {
        let ids: Vec<SoundId> = self.sounds.iter().map(|s| s.id).collect();
        let _ = self.control.remove_input(&ids);
    }
}

/// The writing end of one source's ring, the other end of its
/// [`InputReader`], and where in a device frame the source's channels are.
#[derive(Debug)]
struct InputWriter {
    frames: Producer<Pcm>,
    /// The source's first channel in the device frame.
    first: usize,
    /// How many channels from there: 1 or 2.
    channels: usize,
}

/// The capture side of an input: takes periods of device frames from
/// whoever owns the capture thread and splits them into its sources.
///
/// From `Control::input`, moved into a device's capture callback.
/// Drop the [`Input`] first, then this with the stream that drives it.
#[derive(Debug)]
pub struct InputFeed {
    /// One per source of the layout, in order.
    sources: Vec<InputWriter>,
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
    pub(crate) fn new(layout: &InputLayout, period: usize) -> (InputFeed, Vec<InputReader>) {
        debug_assert!(
            (1..=MAX_PERIOD_FRAMES).contains(&period),
            "the engine checks the period"
        );
        Self::with_ring(layout, period * RING_PERIODS)
    }

    /// [`new`](Self::new) with rings of `frames` frames.
    fn with_ring(layout: &InputLayout, frames: usize) -> (InputFeed, Vec<InputReader>) {
        let mut sources = Vec::with_capacity(layout.entries().len());
        let mut readers = Vec::with_capacity(layout.entries().len());
        let mut first = 0;
        for &source in layout.entries() {
            // This allocation is seen and deliberately not on the ledger.
            // `rtrb` allocates the ring itself, so it could only be counted
            // beside the buffer, not as one; and it does not scale with
            // the audio held, only with the period and the layout, both
            // bounded, so its worst case is `MAX_RING_BYTES` and it is
            // taken as part of the renderer's fixed overhead.
            let (writer, reader) = rtrb::RingBuffer::new(frames * source.count());
            sources.push(InputWriter {
                frames: writer,
                first,
                channels: source.count(),
            });
            readers.push(InputReader::new(reader, source));
            first += source.count();
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
    use super::*;
    use crate::audio::ChannelLayout::{Mono, Stereo};
    use crate::render::dsp::Source;
    use crate::testing::hear;

    /// A feed laid out as `layout` over rings of `frames` frames, and its
    /// readers.
    fn feed(layout: &[ChannelLayout], frames: usize) -> (InputFeed, Vec<InputReader>) {
        InputFeed::with_ring(&InputLayout::new(layout), frames)
    }

    #[test]
    fn a_layout_is_as_wide_as_its_sources() {
        assert_eq!(InputLayout::new([]).width(), 0);
        assert_eq!(InputLayout::new([Mono]).width(), 1);
        assert_eq!(InputLayout::new([Stereo, Mono, Mono]).width(), 4);
    }

    #[test]
    fn a_device_frame_is_split_into_its_sources() {
        // A pair on channels 0 and 1, a microphone on 2, another on 3.
        let (mut f, mut readers) = feed(&[Stereo, Mono, Mono], 8);
        assert_eq!(f.channels(), 4);
        assert_eq!(readers.len(), 3);
        f.feed(&[0.1, 0.2, 0.4, 0.8, 0.11, 0.22, 0.44, 0.88]);

        let mut out = vec![0.0; 4];
        hear(&mut readers[0], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.11, 0.22], "the pair, left and right");
        let mut out = vec![0.0; 4];
        hear(&mut readers[1], &mut out);
        assert_eq!(out, vec![0.4, 0.4, 0.44, 0.44], "the first microphone");
        let mut out = vec![0.0; 4];
        hear(&mut readers[2], &mut out);
        assert_eq!(out, vec![0.8, 0.8, 0.88, 0.88], "the second");
    }

    #[test]
    fn a_full_ring_drops_whole_frames_from_that_source_only() {
        let (mut f, mut readers) = feed(&[Stereo, Mono], 1);
        // One frame fits in each ring; the second frame finds both full.
        f.feed(&[0.1, 0.5, 0.9, 0.2, 0.6, 0.8]);
        let mut out = vec![0.0; 4];
        hear(&mut readers[0], &mut out);
        assert_eq!(out, vec![0.1, 0.5, 0.0, 0.0]);
        let mut out = vec![0.0; 4];
        hear(&mut readers[1], &mut out);
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
    fn new_sizes_each_ring_from_the_period_in_whole_frames_of_its_width() {
        let (mut f, mut readers) = InputFeed::new(&InputLayout::new([Mono, Stereo]), 4);
        assert_eq!(readers[0].capacity(), 4 * RING_PERIODS);
        assert_eq!(readers[1].capacity(), 4 * RING_PERIODS * 2);
        assert_eq!(f.channels(), 3);
        f.feed(&[0.9, 0.1, 0.5]);
        let mut out = vec![0.0; 2];
        hear(&mut readers[1], &mut out);
        assert_eq!(out, vec![0.1, 0.5]);
    }
}

#[cfg(test)]
mod worst_case {
    use super::*;

    #[test]
    fn the_widest_input_at_the_longest_period_takes_exactly_the_worst_case() {
        let widest = [ChannelLayout::Stereo; MAX_INPUT_CHANNELS / 2];
        let (_f, readers) = InputFeed::new(&InputLayout::new(widest), MAX_PERIOD_FRAMES);
        let bytes: usize = readers
            .iter()
            .map(|r| r.capacity() * size_of::<Pcm>())
            .sum();
        assert_eq!(bytes, MAX_RING_BYTES);
    }
}
