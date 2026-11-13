//! Audio in RAM, and the one place its memory comes from.
//!
//! [`AudioMemory`] counts every byte of audio before it exists, against an
//! optional budget. It makes only `AudioBuffer`s, which repay on drop, so
//! release cannot be forgotten or doubled. Whoever holds an `AudioMemory`
//! may allocate, never the render side; the handle inside a buffer only
//! repays, so holding one grants nothing. A buffer has no identity: it
//! becomes a sample, with an id, only when a project stores it.

use std::fmt;
use std::hint;
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

    /// Empty, with room for `frames`, paid for in full now.
    pub fn alloc(&self, layout: ChannelLayout, frames: usize) -> Result<Audio, OverBudgetError> {
        Ok(Audio::new(self.alloc_buffer(layout, frames)?))
    }

    /// Unshared, for the project to fill. Refused, changing nothing, past the
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

/// All figures are bytes.
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

/// Interleaved, at the engine's fixed rate. The capacity is what was paid
/// for and never changes; the audio is the filled part and may be shorter,
/// as for a recording stopped early. It changes only through `&mut` and by
/// whole frames, so a shared buffer never changes.
pub(crate) struct AudioBuffer {
    layout: ChannelLayout,
    /// Never reallocated.
    data: Vec<Pcm>,
    memory: AudioMemory,
}

impl AudioBuffer {
    pub(crate) fn layout(&self) -> ChannelLayout {
        self.layout
    }

    pub(crate) fn frames(&self) -> usize {
        self.data.len() / self.layout.count()
    }

    /// No room left for a whole frame.
    pub(crate) fn is_full(&self) -> bool {
        self.data.capacity() - self.data.len() < self.layout.count()
    }

    /// Panics if the range runs past the filled audio.
    pub(crate) fn frames_in(&self, frames: Range<usize>) -> &[Pcm] {
        let c = self.layout.count();
        &self.data[frames.start * c..frames.end * c]
    }

    fn samples(&self) -> &[Pcm] {
        &self.data
    }

    /// Returns frames written. Stops when full; a part frame at the end is
    /// dropped.
    pub(crate) fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> usize {
        let before = self.frames();
        let capacity = self.data.capacity();
        self.data
            .extend(samples.into_iter().take(capacity - self.data.len()));
        self.data.truncate(self.frames() * self.layout.count());
        debug_assert_eq!(self.data.capacity(), capacity, "a buffer grew");
        self.frames() - before
    }

    pub(crate) fn clear(&mut self) {
        self.data.clear();
    }

    /// Writes the unfilled room once, so the pages behind it are backed.
    /// Fresh memory is only reserved: the first write to each page stalls
    /// while the kernel finds one, which must not happen on the audio thread.
    pub(crate) fn prefault(&mut self) {
        let room = self.data.spare_capacity_mut();
        for sample in room.iter_mut() {
            sample.write(0.0);
        }
        hint::black_box(room);
    }
}

impl Drop for AudioBuffer {
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

    /// Gives up this handle's reference, with no copy left behind.
    pub(crate) fn into_shared(self) -> Arc<AudioBuffer> {
        self.0
    }

    pub fn layout(&self) -> ChannelLayout {
        self.0.layout()
    }

    /// One frame holds a sample per channel.
    pub fn frames(&self) -> usize {
        self.0.frames()
    }

    /// Interleaved.
    pub fn samples(&self) -> &[Pcm] {
        self.0.samples()
    }

    /// Returns frames written, stopping when full and dropping a part frame
    /// at the end. `None`, writing nothing, while the engine holds the
    /// audio: a voice playing it, or one ended and not yet collected.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> Option<usize> {
        Arc::get_mut(&mut self.0).map(|buffer| buffer.extend(samples))
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

    #[test]
    fn a_buffer_is_full_once_no_whole_frame_fits() {
        let mut audio = AudioMemory::new().alloc_buffer(Stereo, 2).unwrap();
        assert!(!audio.is_full());
        audio.extend([0.0; 4]);
        assert!(audio.is_full());
    }

    #[test]
    fn clear_empties_the_buffer_and_keeps_its_room() {
        let mut filled = buffer(Stereo, &[0.5; 4]);
        filled.clear();
        assert_eq!(filled.frames(), 0);
        assert_eq!(filled.extend([0.25; 4]), 2);
    }

    #[test]
    fn prefault_leaves_the_buffer_empty_with_all_its_room() {
        let memory = AudioMemory::new();
        let mut audio = memory.alloc_buffer(Stereo, 4).unwrap();
        audio.prefault();
        assert_eq!(audio.frames(), 0);
        assert_eq!(audio.extend([0.5; 8]), 4);
        assert_eq!(memory.in_use(), 8 * S);
    }
}
