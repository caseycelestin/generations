//! The capture side of an input: the handle a device callback feeds, and
//! the layout that says what the device's channels mean.
//!
//! An input is the ability to put audio into the renderer; whether it is
//! heard is a separate switch, monitoring, which is the input mixer's
//! mute. An input is added in the shape of its device: one layout saying
//! what each channel of the device's interleaved frames means, which are
//! stereo pairs and which are mono. Each mono or stereo entry is a source
//! of its own, with its own [`SoundId`], its own ring and its own slot in
//! the input mixer, monitored on its own. The split from the device's
//! frames into the sources happens here, in [`Input::feed`], where the
//! interleaved frame already is; the render side,
//! [`InputReader`](crate::dsp::InputReader), sees only mono and stereo
//! sources, the same as it sees samples.
//!
//! This is control-side code in the sense of the crate's split: it
//! allocates, at [`Input::open`], paying the ledger for the rings, and it
//! knows what a device is shaped like. The
//! one method the capture thread calls, [`Input::feed`], does neither:
//! it is a fixed loop over the period into rings that already exist, so
//! a device callback with a hard deadline can call it.
//!
//! The rings are made when the input is added and cross to the render
//! side on the commands that add its sources, so there is nothing to park
//! when an input is removed. The layout is fixed then too: a device that
//! is to deliver other channels is closed and reopened, and so is its
//! input. Both ends hold a ring; it is freed when the second goes. Remove
//! the input first and drop the `Input` second, so the free happens on
//! the caller's thread. Dropping the `Input` first finishes every reader,
//! which frees each ring on the audio thread when the mixer lets the
//! slot go.

use std::mem::size_of;
use std::sync::Arc;

use rtrb::Producer;

use crate::config::Pcm;
use crate::dsp::{InputReader, SoundId};
use crate::memory::{Ledger, OverBudget, Reservation};

/// How many of the capture device's periods a source's ring holds: enough
/// that the two clocks can disagree by [`MAX_LAG_PERIODS`] of output and
/// a capture period can land on top before a frame is dropped. The ring
/// is whole frames of the source's width, so a frame never wraps.
const RING_PERIODS: usize = 8;

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

/// Identity of one input: a handle and the layout it was added with. The
/// engine's name for it in `remove_input`; it is not in any mixer. The
/// input's sources are the sounds, and they have [`SoundId`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputId(pub u64);

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
/// From `Engine::input`, moved into a device's capture callback.
/// `Engine::remove_input` it, then drop it with the stream that drives
/// it, before the engine.
#[derive(Debug)]
pub struct Input {
    /// One per source of the layout, in order.
    sources: Vec<SourceWriter>,
    /// Samples per device frame: the layout's width.
    width: usize,
    /// The input as a whole: what `remove_input` takes.
    id: InputId,
    /// One sound per source of the layout, in order: what `monitor` and
    /// `unmonitor` take.
    sounds: Vec<SoundId>,
    /// What the rings cost the budget; repaid when this handle drops.
    _rings: Reservation,
}

impl Input {
    /// Both ends of a new input laid out as `layout`, as the input `id`,
    /// its sources the sounds `sounds`, one per entry of `layout`, in
    /// order, fed `period` frames at a time. The rings are sized from the
    /// period, paid for on `ledger` and allocated here, on the control
    /// side; refused with nothing allocated if they do not fit. The
    /// `Input` goes to the capture callback; each [`InputReader`] crosses
    /// to the render side on the command that adds its source, in the same
    /// order as `sounds`.
    pub(crate) fn open(
        layout: &[Channels],
        period: usize,
        id: InputId,
        sounds: &[SoundId],
        ledger: &Arc<Ledger>,
    ) -> Result<(Input, Vec<InputReader>), OverBudget> {
        Self::open_with(layout, id, sounds, period * RING_PERIODS, ledger)
    }

    /// [`open`](Self::open) with rings of `frames` frames.
    fn open_with(
        layout: &[Channels],
        id: InputId,
        sounds: &[SoundId],
        frames: usize,
        ledger: &Arc<Ledger>,
    ) -> Result<(Input, Vec<InputReader>), OverBudget> {
        debug_assert_eq!(layout.len(), sounds.len(), "a sound per source");
        // Paid for before anything is allocated, all the rings at once.
        let rings = ledger.reserve(frames * width(layout) * size_of::<Pcm>())?;
        let mut sources = Vec::with_capacity(layout.len());
        let mut readers = Vec::with_capacity(layout.len());
        let mut first = 0;
        for &channels in layout {
            let (writer, reader) = rtrb::RingBuffer::new(frames * channels.count());
            sources.push(SourceWriter {
                frames: writer,
                first,
                channels: channels.count(),
            });
            readers.push(InputReader::new(reader, channels.count() as u16));
            first += channels.count();
        }
        let input = Input {
            sources,
            width: first,
            id,
            sounds: sounds.to_vec(),
            _rings: rings,
        };
        Ok((input, readers))
    }

    /// The input as a whole: what `Engine::remove_input` takes.
    pub fn id(&self) -> InputId {
        self.id
    }

    /// One sound per source of the layout the input was added with, in
    /// order: what `Engine::monitor` and `unmonitor` take.
    pub fn sounds(&self) -> &[SoundId] {
        &self.sounds
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
    use crate::testing::unlimited;

    /// An input laid out as `layout`, its sources the sounds 1, 2, ...,
    /// over rings of `frames` frames.
    fn input(layout: &[Channels], frames: usize) -> (Input, Vec<InputReader>) {
        let sounds: Vec<SoundId> = (1..=layout.len() as u64).map(SoundId).collect();
        Input::open_with(layout, InputId(0), &sounds, frames, &unlimited()).unwrap()
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
        let (mut i, mut readers) = input(&[Stereo, Mono, Mono], 8);
        assert_eq!(i.channels(), 4);
        assert_eq!(i.sounds(), [SoundId(1), SoundId(2), SoundId(3)]);
        i.feed(&[0.1, 0.2, 0.4, 0.8, 0.11, 0.22, 0.44, 0.88]);

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
        let (mut i, mut readers) = input(&[Stereo, Mono], 1);
        // One frame fits in each ring; the second frame finds both full.
        i.feed(&[0.1, 0.5, 0.9, 0.2, 0.6, 0.8]);
        let mut out = vec![0.0; 4];
        readers[0].add(&mut out);
        assert_eq!(out, vec![0.1, 0.5, 0.0, 0.0]);
        let mut out = vec![0.0; 4];
        readers[1].add(&mut out);
        assert_eq!(out, vec![0.9, 0.9, 0.0, 0.0]);
    }

    #[test]
    fn dropping_the_input_finishes_every_reader() {
        let (i, readers) = input(&[Stereo, Mono], 8);
        assert!(readers.iter().all(|r| !r.finished()));
        drop(i);
        assert!(readers.iter().all(|r| r.finished()));
    }

    #[test]
    fn open_sizes_each_ring_from_the_period_in_whole_frames_of_its_width() {
        let (mut i, mut readers) = Input::open(
            &[Mono, Stereo],
            4,
            InputId(5),
            &[SoundId(6), SoundId(7)],
            &unlimited(),
        )
        .unwrap();
        assert_eq!(readers[0].capacity(), 4 * RING_PERIODS);
        assert_eq!(readers[1].capacity(), 4 * RING_PERIODS * 2);
        assert_eq!(i.id(), InputId(5));
        assert_eq!(i.sounds(), [SoundId(6), SoundId(7)]);
        assert_eq!(i.channels(), 3);
        i.feed(&[0.9, 0.1, 0.5]);
        let mut out = vec![0.0; 2];
        readers[1].add(&mut out);
        assert_eq!(out, vec![0.1, 0.5]);
    }
}

#[cfg(test)]
mod budget_tests {
    use std::mem::size_of;

    use super::Channels::{Mono, Stereo};
    use super::*;
    use crate::dsp::Source;
    use crate::memory::Ledger;

    #[test]
    fn the_rings_are_paid_for_on_the_ledger_and_repaid_when_the_input_drops() {
        let ledger = Ledger::new(usize::MAX);
        let (i, readers) = Input::open(
            &[Stereo, Mono],
            4,
            InputId(0),
            &[SoundId(1), SoundId(2)],
            &ledger,
        )
        .unwrap();
        // Three channels, four frames a period, eight periods a ring.
        assert_eq!(ledger.in_use(), 3 * 4 * RING_PERIODS * size_of::<Pcm>());
        drop(i);
        assert_eq!(ledger.in_use(), 0, "repaid when the handle drops");
        assert!(readers.iter().all(|r| r.finished()));
    }

    #[test]
    fn an_input_over_budget_is_refused_with_nothing_allocated() {
        let ledger = Ledger::new(10);
        let refused = Input::open(&[Mono], 4, InputId(0), &[SoundId(1)], &ledger);
        assert!(refused.is_err());
        assert_eq!(ledger.in_use(), 0);
    }
}
