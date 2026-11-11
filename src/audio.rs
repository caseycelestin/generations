//! Project audio over a `Block` from the memory ledger.
//!
//! An [`AudioBuffer`] is a block of [`Pcm`] samples with a channel count:
//! immutable, interleaved, at the project's fixed rate, with no identity
//! and no idea where it came from. The block is where the memory was paid
//! for, so a buffer is never uncounted and cannot be copied without going
//! back to the ledger for another block. The sample type is the one the
//! render side mixes in, so a read is a slice and nothing converts.
//!

use std::fmt;
use std::ops::Range;

use thiserror::Error;

use crate::config::{MAX_CHANNELS, Pcm};
use crate::memory::Block;

/// Reasons a buffer cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AudioError {
    /// `channels` was zero or above [`MAX_CHANNELS`].
    #[error("channel count {0} is not in 1..={MAX_CHANNELS}")]
    BadChannelCount(u16),
    /// The sample count was not a whole number of frames.
    #[error("{samples} samples is not a whole number of {channels}-channel frames")]
    RaggedFrames { samples: usize, channels: u16 },
}

/// Interleaved [`Pcm`] audio at the engine's fixed sample rate, in a block
/// paid for against the ledger.
///
/// The audio is the block's length; the block's capacity is what was
/// paid for and may be larger, as it is for a take that stopped before
/// its block was full. Both stay as they are for the buffer's life: the
/// audio never changes and the bytes return when the block drops.
pub struct AudioBuffer {
    channels: u16,
    data: Block<Pcm>,
}

impl AudioBuffer {
    /// Wraps a block of interleaved samples, checking the channel count
    /// and that the block holds whole frames.
    ///
    /// Refused audio hands the block back beside the error: nothing is
    /// freed here, so a caller that wants to try again with a different
    /// channel count still holds the memory.
    pub fn new(channels: u16, data: Block<Pcm>) -> Result<Self, (AudioError, Block<Pcm>)> {
        if let Err(e) = check_channels(channels) {
            return Err((e, data));
        }
        if !data.len().is_multiple_of(usize::from(channels)) {
            return Err((
                AudioError::RaggedFrames {
                    samples: data.len(),
                    channels,
                },
                data,
            ));
        }
        Ok(Self { channels, data })
    }

    /// Number of interleaved channels.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Number of frames, where one frame holds one sample per channel.
    pub fn frames(&self) -> usize {
        self.data.len() / usize::from(self.channels)
    }

    /// The samples of frames `frames`, interleaved. This is how playback
    /// reads: a period's worth at a time, straight from the block.
    ///
    /// Panics if the range runs past the end.
    pub fn frames_in(&self, frames: Range<usize>) -> &[Pcm] {
        let c = usize::from(self.channels);
        &self.data[frames.start * c..frames.end * c]
    }

    /// All samples, interleaved.
    pub fn samples(&self) -> &[Pcm] {
        &self.data
    }
}

/// Checks a channel count against what a buffer may hold.
pub(crate) fn check_channels(channels: u16) -> Result<(), AudioError> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::BadChannelCount(channels));
    }
    Ok(())
}

impl fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBuffer")
            .field("channels", &self.channels)
            .field("frames", &self.frames())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::memory::Ledger;

    /// A block holding `data`, from a ledger that refuses nothing.
    fn block(data: &[Pcm]) -> (Ledger, Block<Pcm>) {
        let ledger = Ledger::new(usize::MAX);
        let mut block = ledger.alloc(data.len()).unwrap();
        block.extend(data.iter().copied());
        (ledger, block)
    }

    fn buffer(channels: u16, data: &[Pcm]) -> AudioBuffer {
        AudioBuffer::new(channels, block(data).1).unwrap()
    }

    #[test]
    fn frames_count_per_channel() {
        let b = buffer(2, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(b.frames(), 2);
        assert_eq!(b.channels(), 2);
        assert_eq!(b.samples(), &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn frames_in_reads_a_range_of_frames() {
        let b = buffer(2, &[0.0, 0.5, -0.5, 1.0, 0.0, 0.0]);
        assert_eq!(b.frames_in(1..2), &[-0.5, 1.0]);
        assert_eq!(b.frames_in(0..3).len(), 6);
        assert_eq!(b.frames_in(3..3).len(), 0);
    }

    #[test]
    fn the_audio_is_the_length_and_the_payment_is_the_capacity() {
        let ledger = Ledger::new(usize::MAX);
        let mut block: Block<Pcm> = ledger.alloc(10).unwrap();
        block.extend([1.0, 2.0, 3.0, 4.0]);
        let b = AudioBuffer::new(1, block).unwrap();
        assert_eq!(b.frames(), 4);
        assert_eq!(ledger.in_use(), 10 * size_of::<Pcm>());
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn rejects_bad_channel_counts_and_keeps_the_block() {
        let (ledger, block) = block(&[]);
        let (err, block) = AudioBuffer::new(0, block).unwrap_err();
        assert_eq!(err, AudioError::BadChannelCount(0));
        let (err, _block) = AudioBuffer::new(3, block).unwrap_err();
        assert_eq!(err, AudioError::BadChannelCount(3));
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn rejects_ragged_frames_and_keeps_the_block() {
        let (ledger, block) = block(&[0.0, 0.0, 0.0]);
        let (err, block) = AudioBuffer::new(2, block).unwrap_err();
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
    fn debug_shows_shape_not_samples() {
        let b = buffer(2, &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            format!("{b:?}"),
            "AudioBuffer { channels: 2, frames: 2, .. }"
        );
    }
}
