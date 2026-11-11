//! Project audio over a `Block` from the memory ledger.
//!
//! An [`AudioBuffer`] is a block of [`Pcm`] samples with a
//! [`ChannelLayout`]: interleaved, at the project's fixed rate, with no
//! identity and no idea where it came from. Only its owner can write to
//! it, a recorder filling a take; shared, as a loaded sample is behind an
//! `Arc`, it never changes. The block is where the
//! memory was paid for, so a buffer is never uncounted and cannot be copied
//! without going back to the ledger for another block. The sample type is
//! the one the render side mixes in, so a read is a slice and nothing
//! converts.
//!
//! Audio in this crate is mono or stereo, and [`ChannelLayout`] says
//! which, for a buffer and for one source of an input alike. A raw channel
//! count exists only where a file or a decoder hands one over, and becomes
//! a layout there or is refused.

use std::fmt;
use std::ops::Range;

use thiserror::Error;

use crate::config::{MAX_CHANNELS, Pcm};
use crate::memory::Block;

/// Reasons a buffer cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AudioError {
    /// A channel count was zero or above [`MAX_CHANNELS`]: no layout.
    #[error("channel count {0} is not in 1..={MAX_CHANNELS}")]
    BadChannelCount(u16),
    /// The sample count was not a whole number of frames.
    #[error("{samples} samples is not a whole number of {channels}-channel frames")]
    RaggedFrames { samples: usize, channels: u16 },
}

/// How many channels a frame holds and what they mean: one heard on both
/// sides, or two heard left and right. Interleaved stereo is left then
/// right everywhere, so a pair is always the two channels in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
}

impl ChannelLayout {
    /// How many channels a frame of this layout holds.
    pub fn count(self) -> usize {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
}

/// The layout of a raw channel count, as a file or decoder gives one.
impl TryFrom<u16> for ChannelLayout {
    type Error = AudioError;

    fn try_from(channels: u16) -> Result<Self, AudioError> {
        match channels {
            1 => Ok(ChannelLayout::Mono),
            2 => Ok(ChannelLayout::Stereo),
            _ => Err(AudioError::BadChannelCount(channels)),
        }
    }
}

/// The raw channel count of a layout, as a file header wants one.
impl From<ChannelLayout> for u16 {
    fn from(layout: ChannelLayout) -> u16 {
        match layout {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
}

/// Interleaved [`Pcm`] audio at the engine's fixed sample rate, in a block
/// paid for against the ledger.
///
/// The audio is the block's length; the block's capacity is what was
/// paid for and may be larger, as it is for a take that stopped before
/// its block was full. The capacity stays as it is for the buffer's life
/// and the bytes return when the block drops. The audio changes only
/// through `&mut`, by whole frames, so a buffer that is shared never
/// changes and a buffer is always whole frames.
pub struct AudioBuffer {
    layout: ChannelLayout,
    data: Block<Pcm>,
}

impl AudioBuffer {
    /// Wraps a block of interleaved samples, checking that it holds whole
    /// frames of `layout`.
    ///
    /// Refused audio hands the block back beside the error: nothing is
    /// freed here, so a caller that wants to try again with a different
    /// layout still holds the memory.
    pub fn new(layout: ChannelLayout, data: Block<Pcm>) -> Result<Self, (AudioError, Block<Pcm>)> {
        if !data.len().is_multiple_of(layout.count()) {
            return Err((
                AudioError::RaggedFrames {
                    samples: data.len(),
                    channels: layout.into(),
                },
                data,
            ));
        }
        Ok(Self { layout, data })
    }

    /// Mono or stereo.
    pub fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Number of frames, where one frame holds one sample per channel.
    pub fn frames(&self) -> usize {
        self.data.len() / self.layout.count()
    }

    /// The samples of frames `frames`, interleaved. This is how playback
    /// reads: a period's worth at a time, straight from the block.
    ///
    /// Panics if the range runs past the end.
    pub fn frames_in(&self, frames: Range<usize>) -> &[Pcm] {
        let c = self.layout.count();
        &self.data[frames.start * c..frames.end * c]
    }

    /// All samples, interleaved.
    pub fn samples(&self) -> &[Pcm] {
        &self.data
    }

    /// How many more frames fit before the block is full.
    pub(crate) fn room(&self) -> usize {
        self.data.room() / self.layout.count()
    }

    /// Appends one frame, a sample per channel, or drops it if there is
    /// no room for the whole frame. Returns whether it went in.
    pub(crate) fn push_frame(&mut self, frame: &[Pcm]) -> bool {
        debug_assert_eq!(frame.len(), self.layout.count(), "a frame of the layout");
        if self.room() == 0 {
            return false;
        }
        self.data.extend(frame.iter().copied());
        true
    }

    /// Empties the buffer. The room comes back; the block stays paid for.
    pub(crate) fn clear(&mut self) {
        self.data.truncate(0);
    }
}

/// A finished take: what a [`Recorder`](crate::Recorder) kept, before
/// it is saved.
///
/// It is the recorder's own buffer, so it lasts until the recorder
/// records again, which records over it. Save it with
/// [`Project::save_take`](crate::Project::save_take) to keep it: that
/// makes it a sample, with an id, that can be loaded and played. The
/// audio stays inside the crate, as a loaded sample's does.
#[derive(Debug)]
pub struct Take(AudioBuffer);

impl Take {
    /// The take held in `audio`.
    pub(crate) fn new(audio: AudioBuffer) -> Self {
        Self(audio)
    }

    /// The audio, to read.
    pub(crate) fn audio(&self) -> &AudioBuffer {
        &self.0
    }

    /// The buffer, to record into again.
    pub(crate) fn into_audio(self) -> AudioBuffer {
        self.0
    }
}

impl fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBuffer")
            .field("layout", &self.layout)
            .field("frames", &self.frames())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelLayout::{Mono, Stereo};
    use super::*;
    use crate::memory::Ledger;

    /// A block holding `data`, from a ledger that refuses nothing.
    fn block(data: &[Pcm]) -> (Ledger, Block<Pcm>) {
        let ledger = Ledger::new(usize::MAX);
        let mut block = ledger.alloc(data.len()).unwrap();
        block.extend(data.iter().copied());
        (ledger, block)
    }

    fn buffer(layout: ChannelLayout, data: &[Pcm]) -> AudioBuffer {
        AudioBuffer::new(layout, block(data).1).unwrap()
    }

    #[test]
    fn frames_count_per_channel() {
        let b = buffer(Stereo, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(b.frames(), 2);
        assert_eq!(b.layout(), Stereo);
        assert_eq!(b.samples(), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn frames_in_reads_a_range_of_frames() {
        let b = buffer(Stereo, &[0.0, 0.5, -0.5, 1.0, 0.0, 0.0]);
        assert_eq!(b.frames_in(1..2), &[-0.5, 1.0]);
        assert_eq!(b.frames_in(0..3).len(), 6);
        assert_eq!(b.frames_in(3..3).len(), 0);
    }

    #[test]
    fn the_audio_is_the_length_and_the_payment_is_the_capacity() {
        let ledger = Ledger::new(usize::MAX);
        let mut block: Block<Pcm> = ledger.alloc(10).unwrap();
        block.extend([1.0, 2.0, 3.0, 4.0]);
        let b = AudioBuffer::new(Mono, block).unwrap();
        assert_eq!(b.frames(), 4);
        assert_eq!(ledger.in_use(), 10 * size_of::<Pcm>());
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_channel_count_is_a_layout_or_refused() {
        assert_eq!(ChannelLayout::try_from(1), Ok(Mono));
        assert_eq!(ChannelLayout::try_from(2), Ok(Stereo));
        assert_eq!(
            ChannelLayout::try_from(0),
            Err(AudioError::BadChannelCount(0))
        );
        assert_eq!(
            ChannelLayout::try_from(3),
            Err(AudioError::BadChannelCount(3))
        );
        assert_eq!(u16::from(Stereo), 2);
    }

    #[test]
    fn rejects_ragged_frames_and_keeps_the_block() {
        let (ledger, block) = block(&[0.0, 0.0, 0.0]);
        let (err, block) = AudioBuffer::new(Stereo, block).unwrap_err();
        assert_eq!(
            err,
            AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            }
        );
        assert_eq!(ledger.in_use(), 3 * size_of::<Pcm>());
        drop(block);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn an_owner_writes_whole_frames_up_to_the_room_and_clears() {
        let ledger = Ledger::new(usize::MAX);
        let block: Block<Pcm> = ledger.alloc(5).unwrap();
        let mut b = AudioBuffer::new(Stereo, block).unwrap();
        assert_eq!(b.room(), 2);
        assert!(b.push_frame(&[1.0, 2.0]));
        assert!(b.push_frame(&[3.0, 4.0]));
        assert!(!b.push_frame(&[5.0, 6.0]), "half a frame of room is none");
        assert_eq!(b.samples(), &[1.0, 2.0, 3.0, 4.0]);
        b.clear();
        assert_eq!(b.frames(), 0);
        assert_eq!(b.room(), 2);
        assert_eq!(ledger.in_use(), 5 * size_of::<Pcm>());
    }

    #[test]
    fn debug_shows_shape_not_samples() {
        let b = buffer(Stereo, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            format!("{b:?}"),
            "AudioBuffer { layout: Stereo, frames: 2, .. }"
        );
    }
}
