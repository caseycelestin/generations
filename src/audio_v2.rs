//! Project audio over a [`Block`].
//!
//! An [`AudioBuffer`] is a block of [`PcmV2`] samples with a channel count:
//! immutable, interleaved, at the project's fixed rate, with no identity
//! and no idea where it came from. The block is where the memory was paid
//! for, so a buffer is never uncounted and cannot be copied without going
//! back to the ledger for another block. The sample type is the one the
//! render side mixes in, so a read is a slice and nothing converts.
//!
//! Replaces `audio::AudioBuffer`, which holds `i16` in a `Vec` paid for by
//! a reservation beside it, and converts to `f32` on every read. Not yet
//! wired to a caller; this file becomes `audio` when the last one ports.

#![allow(dead_code)] // until the first caller is ported

use std::fmt;
use std::ops::Range;

use crate::audio::{AudioError, check_channels};
use crate::config::PcmV2;
use crate::memory::Block;

/// Interleaved [`PcmV2`] audio at the engine's fixed sample rate, in a block
/// paid for against the ledger.
///
/// The audio is the block's length; the block's capacity is what was
/// paid for and may be larger, as it is for a take that stopped before
/// its block was full. Both stay as they are for the buffer's life: the
/// audio never changes and the bytes return when the block drops.
pub struct AudioBuffer {
    channels: u16,
    data: Block<PcmV2>,
}

impl AudioBuffer {
    /// Wraps a block of interleaved samples, checking the channel count
    /// and that the block holds whole frames.
    ///
    /// Refused audio hands the block back beside the error: nothing is
    /// freed here, so a caller that wants to try again with a different
    /// channel count still holds the memory.
    pub fn new(channels: u16, data: Block<PcmV2>) -> Result<Self, (AudioError, Block<PcmV2>)> {
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
    pub fn frames_in(&self, frames: Range<usize>) -> &[PcmV2] {
        let c = usize::from(self.channels);
        &self.data[frames.start * c..frames.end * c]
    }

    /// All samples, interleaved.
    pub fn samples(&self) -> &[PcmV2] {
        &self.data
    }

    /// The block back, as it came in. Nothing is copied or freed: the
    /// memory stays paid for and can be filled again.
    pub fn into_block(self) -> Block<PcmV2> {
        self.data
    }
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
    use std::sync::Arc;

    use super::*;
    use crate::memory::Ledger;

    /// A block holding `data`, from a ledger that refuses nothing.
    fn block(data: &[PcmV2]) -> (Arc<Ledger>, Block<PcmV2>) {
        let ledger = Ledger::new(usize::MAX);
        let mut block = ledger.alloc(data.len()).unwrap();
        block.extend(data.iter().copied());
        (ledger, block)
    }

    fn buffer(channels: u16, data: &[PcmV2]) -> AudioBuffer {
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
        let mut block: Block<PcmV2> = ledger.alloc(10).unwrap();
        block.extend([1.0, 2.0, 3.0, 4.0]);
        let b = AudioBuffer::new(1, block).unwrap();
        assert_eq!(b.frames(), 4);
        assert_eq!(ledger.in_use(), 10 * size_of::<PcmV2>());
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn into_block_gives_the_memory_back_unfreed() {
        let (ledger, block) = block(&[1.0, 2.0]);
        let before = block.as_ptr();
        let b = AudioBuffer::new(1, block).unwrap();
        let block = b.into_block();
        assert_eq!(block.as_ptr(), before);
        assert_eq!(ledger.in_use(), 2 * size_of::<PcmV2>());
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
        assert_eq!(ledger.in_use(), 3 * size_of::<PcmV2>());
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
