//! Audio in RAM, and the one place its memory comes from.
//!
//! [`AudioMemory`] counts every byte of audio before it exists, against
//! an optional budget. An `AudioBuffer` is the only thing it makes, and
//! repays on drop, so release cannot be forgotten or doubled. Whoever holds
//! an `AudioMemory` may allocate: the caller and what it lends it to, never
//! the render side. A buffer's handle is private and only repays, so
//! holding one grants nothing. Users hold a buffer only as an [`Audio`].
//!
//! A buffer is interleaved [`Pcm`] at the project's fixed rate, with no
//! identity and no idea where it came from: it becomes a sample, with an
//! id, only when a project stores it. The sample type is the one the render
//! side mixes in, so a read is a slice and nothing converts.

use std::fmt;
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

use crate::config::Pcm;
use crate::layout::ChannelLayout;

/// Make one for the machine and hand it to everything that brings audio
/// into RAM. A handle: clones share one figure, and a buffer repays it even
/// after every handle is gone.
#[derive(Debug, Clone)]
pub struct AudioMemory(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    budget: usize,
    /// Protects nothing but itself, so relaxed ordering is enough.
    in_use: AtomicUsize,
}

impl AudioMemory {
    /// Counts every byte and refuses only a count that overflows.
    pub fn new() -> Self {
        Self::with_budget(usize::MAX)
    }

    /// Refuses past `budget` bytes in use at once.
    pub fn with_budget(budget: usize) -> Self {
        Self(Arc::new(Shared {
            budget,
            in_use: AtomicUsize::new(0),
        }))
    }

    /// An empty sample with room for `frames`, paid for in full now.
    pub fn alloc(&self, layout: ChannelLayout, frames: usize) -> Result<Audio, OverBudgetError> {
        Ok(Audio::new(self.alloc_buffer(layout, frames)?))
    }

    /// Unshared, for storage to fill. Refused, changing nothing, past the
    /// budget (reaching it is allowed) or when the byte count overflows.
    pub(crate) fn alloc_buffer(
        &self,
        layout: ChannelLayout,
        frames: usize,
    ) -> Result<AudioBuffer, OverBudgetError> {
        let shared = &*self.0;
        let over = |requested, in_use| OverBudgetError {
            requested,
            in_use,
            budget: shared.budget,
        };
        let bytes = frames
            .checked_mul(layout.count())
            .and_then(|samples| samples.checked_mul(size_of::<Pcm>()))
            .ok_or_else(|| over(usize::MAX, shared.in_use.load(Ordering::Relaxed)))?;
        shared
            .in_use
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                in_use
                    .checked_add(bytes)
                    .filter(|&total| total <= shared.budget)
            })
            .map_err(|in_use| over(bytes, in_use))?;
        Ok(AudioBuffer {
            layout,
            data: Vec::with_capacity(frames * layout.count()),
            memory: self.clone(),
        })
    }

    #[cfg(test)] // read by nothing but tests until a panel asks
    pub(crate) fn in_use(&self) -> usize {
        self.0.in_use.load(Ordering::Relaxed)
    }

    /// Only a dropping buffer calls this.
    fn release(&self, bytes: usize) {
        self.0.in_use.fetch_sub(bytes, Ordering::Relaxed);
    }
}

/// No budget, as [`AudioMemory::new`].
impl Default for AudioMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// An allocation refused for the budget. All figures are bytes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{requested} bytes would put audio memory at {} of a {budget} byte budget",
    in_use.saturating_add(*requested)
)]
pub struct OverBudgetError {
    pub requested: usize,
    pub in_use: usize,
    pub budget: usize,
}

/// Interleaved [`Pcm`] audio at the engine's fixed sample rate, paid for
/// against [`AudioMemory`].
///
/// The capacity is what was paid for and never changes; the audio is the
/// filled part and may be shorter, as for a take stopped early. The audio
/// changes only through `&mut`, by whole frames, so a buffer that is
/// shared never changes and a buffer is always whole frames.
pub(crate) struct AudioBuffer {
    layout: ChannelLayout,
    /// Never reallocated.
    data: Vec<Pcm>,
    memory: AudioMemory,
}

impl AudioBuffer {
    /// Mono or stereo.
    pub fn layout(&self) -> ChannelLayout {
        self.layout
    }

    /// Number of frames, where one frame holds one sample per channel.
    pub fn frames(&self) -> usize {
        self.data.len() / self.layout.count()
    }

    /// The samples of frames `frames`, interleaved. This is how playback
    /// reads: a period's worth at a time, straight from the buffer.
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

    /// Appends interleaved samples until the buffer is full or `samples`
    /// runs out, and returns how many frames went in. A part frame at the
    /// end is dropped, so the buffer stays whole frames.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> usize {
        let before = self.frames();
        let capacity = self.data.capacity();
        self.data
            .extend(samples.into_iter().take(capacity - self.data.len()));
        self.data.truncate(self.frames() * self.layout.count());
        debug_assert_eq!(self.data.capacity(), capacity, "a buffer grew");
        self.frames() - before
    }
}

impl Drop for AudioBuffer {
    /// The capacity, not the length: that is what was paid for.
    fn drop(&mut self) {
        self.memory.release(self.data.capacity() * size_of::<Pcm>());
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

/// Audio in RAM, shared with whatever plays it, and written only while
/// nothing does. Holding it is what loaded means: drop it and its memory
/// is repaid once nothing playing it holds the audio.
#[derive(Debug)]
pub struct Audio(Arc<AudioBuffer>);

impl Audio {
    pub(crate) fn new(buffer: AudioBuffer) -> Self {
        Self(Arc::new(buffer))
    }

    /// Mono or stereo.
    pub fn layout(&self) -> ChannelLayout {
        self.0.layout()
    }

    /// Number of frames, where one frame holds one sample per channel.
    pub fn frames(&self) -> usize {
        self.0.frames()
    }

    /// All samples, interleaved.
    pub fn samples(&self) -> &[Pcm] {
        self.0.samples()
    }

    /// Appends interleaved samples until the audio is full or `samples`
    /// runs out, and returns how many frames went in; a part frame at the
    /// end is dropped. `None`, writing nothing, while the engine holds it:
    /// a voice playing it, or one that ended and has not been collected.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> Option<usize> {
        Arc::get_mut(&mut self.0).map(|buffer| buffer.extend(samples))
    }

    pub(crate) fn buffer(&self) -> &AudioBuffer {
        &self.0
    }

    /// What a playing of it holds.
    pub(crate) fn shared(&self) -> &Arc<AudioBuffer> {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelLayout::{Mono, Stereo};
    use super::*;
    use crate::testing::buffer;

    /// Bytes of one sample.
    const S: usize = size_of::<Pcm>();

    #[test]
    fn new_memory_has_nothing_in_use() {
        assert_eq!(AudioMemory::with_budget(100).in_use(), 0);
    }

    #[test]
    fn alloc_pays_for_the_room_and_holds_nothing() {
        let memory = AudioMemory::new();
        let audio = memory.alloc(Stereo, 4).unwrap();
        assert_eq!(audio.frames(), 0);
        assert_eq!(memory.in_use(), 8 * S);
        drop(audio);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn a_buffer_counts_until_it_is_dropped() {
        let memory = AudioMemory::with_budget(100 * S);
        let a = memory.alloc_buffer(Mono, 15).unwrap();
        let b = memory.alloc_buffer(Mono, 5).unwrap();
        assert_eq!(memory.in_use(), 20 * S);
        drop(a);
        assert_eq!(memory.in_use(), 5 * S);
        drop(b);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn exactly_the_budget_is_allowed() {
        let memory = AudioMemory::with_budget(100 * S);
        let _a = memory.alloc_buffer(Mono, 30).unwrap();
        let _b = memory.alloc_buffer(Stereo, 35).unwrap();
        assert_eq!(memory.in_use(), 100 * S);
        assert!(memory.alloc_buffer(Mono, 1).is_err());
    }

    #[test]
    fn over_budget_is_refused_and_changes_nothing() {
        let memory = AudioMemory::with_budget(50 * S);
        let _a = memory.alloc_buffer(Mono, 40).unwrap();
        let err = memory.alloc(Stereo, 10).unwrap_err();
        assert_eq!(
            err,
            OverBudgetError {
                requested: 20 * S,
                in_use: 40 * S,
                budget: 50 * S,
            }
        );
        assert_eq!(memory.in_use(), 40 * S);
    }

    #[test]
    fn a_count_whose_bytes_overflow_is_refused_as_unaffordable() {
        let memory = AudioMemory::new();
        let err = memory.alloc(Stereo, usize::MAX).unwrap_err();
        assert_eq!(err.requested, usize::MAX);
        let err = memory.alloc(Mono, usize::MAX).unwrap_err();
        assert_eq!(err.requested, usize::MAX);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn a_buffer_outlives_the_memory_that_made_it() {
        let memory = AudioMemory::new();
        let a = memory.alloc_buffer(Mono, 10).unwrap();
        drop(memory);
        drop(a); // repays into the figure it still holds; must not panic
    }

    #[test]
    fn the_audio_is_the_length_and_the_payment_is_the_capacity() {
        let memory = AudioMemory::new();
        let mut b = memory.alloc_buffer(Mono, 10).unwrap();
        b.extend([1.0, 2.0, 3.0, 4.0]);
        assert_eq!(b.frames(), 4);
        assert_eq!(memory.in_use(), 10 * S);
        drop(b);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn extend_takes_whole_frames_up_to_the_room() {
        let memory = AudioMemory::new();
        let mut b = memory.alloc_buffer(Stereo, 2).unwrap();
        assert_eq!(b.extend([1.0, 2.0, 3.0]), 1);
        assert_eq!(b.samples(), &[1.0, 2.0]);
        assert_eq!(b.extend([5.0, 6.0, 7.0, 8.0]), 1);
        assert_eq!(b.samples(), &[1.0, 2.0, 5.0, 6.0]);
        assert_eq!(b.extend([9.0, 10.0]), 0);
        assert_eq!(memory.in_use(), 4 * S);
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
    fn audio_is_written_only_while_nothing_else_holds_it() {
        let mut audio = AudioMemory::new().alloc(Mono, 2).unwrap();
        let playing = Arc::clone(audio.shared());
        assert_eq!(audio.extend([0.5]), None);
        drop(playing);
        assert_eq!(audio.extend([0.5]), Some(1));
        assert_eq!(audio.samples(), &[0.5]);
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
