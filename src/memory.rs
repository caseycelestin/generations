//! The only source of audio-length memory, with an optional budget.
//!
//! A [`Block`] is paid for when [`SampleMemory`] makes it and repays on
//! drop, so every audio byte is counted before it exists and release cannot
//! be forgotten or doubled. Who may allocate is who holds a `SampleMemory`:
//! the project, never the render side. A block keeps a private handle only
//! to repay, so holding one grants nothing.

use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

use crate::config::Pcm;

/// A handle: clones share one figure, and a block repays it even after
/// the project is gone.
#[derive(Debug, Clone)]
pub(crate) struct SampleMemory(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    budget: usize,
    /// Protects nothing but itself, so relaxed ordering is enough.
    in_use: AtomicUsize,
}

impl SampleMemory {
    /// Counts every byte and refuses only a count that overflows.
    #[allow(dead_code, reason = "used once the caller makes its own memory")]
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

    /// An empty block with room for exactly `samples`. Refused, changing
    /// nothing, past the budget (reaching it is allowed) or when the byte
    /// count overflows.
    pub fn alloc(&self, samples: usize) -> Result<Block, OverBudgetError> {
        let shared = &*self.0;
        let over = |requested, in_use| OverBudgetError {
            requested,
            in_use,
            budget: shared.budget,
        };
        let bytes = samples
            .checked_mul(size_of::<Pcm>())
            .ok_or_else(|| over(usize::MAX, shared.in_use.load(Ordering::Relaxed)))?;
        shared
            .in_use
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                in_use
                    .checked_add(bytes)
                    .filter(|&total| total <= shared.budget)
            })
            .map_err(|in_use| over(bytes, in_use))?;
        Ok(Block {
            data: Vec::with_capacity(samples),
            memory: self.clone(),
        })
    }

    #[cfg(test)] // read by nothing but tests until a panel asks
    pub fn in_use(&self) -> usize {
        self.0.in_use.load(Ordering::Relaxed)
    }

    /// Only a dropping block calls this.
    fn release(&self, bytes: usize) {
        self.0.in_use.fetch_sub(bytes, Ordering::Relaxed);
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

/// A fixed-capacity buffer of samples paid for against [`SampleMemory`]: the
/// one shape audio-length memory takes.
///
/// The capacity never changes, so what was paid for is what is held; the
/// length is how much is filled.
#[derive(Debug)]
pub struct Block {
    /// Never reallocated.
    data: Vec<Pcm>,
    memory: SampleMemory,
}

impl Block {
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    pub fn room(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Drops the sample if the block is full; returns whether it went in.
    pub fn push(&mut self, sample: Pcm) -> bool {
        if self.room() == 0 {
            return false;
        }
        self.data.push(sample);
        true
    }

    /// Takes what fits, drops the rest, and returns how many went in.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> usize {
        let before = self.len();
        let room = self.room();
        self.data.extend(samples.into_iter().take(room));
        debug_assert_eq!(self.data.capacity(), self.capacity(), "a block grew");
        self.len() - before
    }

    /// As `Vec::truncate`; the capacity stays paid for.
    pub fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }
}

/// Reads as the filled part.
impl Deref for Block {
    type Target = [Pcm];

    fn deref(&self) -> &[Pcm] {
        &self.data
    }
}

/// Writes the filled part in place; the length cannot change through it.
impl DerefMut for Block {
    fn deref_mut(&mut self) -> &mut [Pcm] {
        &mut self.data
    }
}

impl Drop for Block {
    /// The capacity, not the length: that is what was paid for.
    fn drop(&mut self) {
        self.memory.release(self.data.capacity() * size_of::<Pcm>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes of one sample.
    const S: usize = size_of::<Pcm>();

    #[test]
    fn new_memory_has_nothing_in_use() {
        let memory = SampleMemory::with_budget(100);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn an_alloc_of_exactly_what_is_left_succeeds() {
        let memory = SampleMemory::with_budget(100 * S);
        let _a = memory.alloc(30).unwrap();
        let _b = memory.alloc(70).unwrap();
        assert_eq!(memory.in_use(), 100 * S);
    }

    #[test]
    fn a_block_counts_until_it_is_dropped() {
        let memory = SampleMemory::with_budget(100 * S);
        let a = memory.alloc(15).unwrap();
        let b = memory.alloc(5).unwrap();
        assert_eq!(memory.in_use(), 20 * S);
        drop(a);
        assert_eq!(memory.in_use(), 5 * S);
        drop(b);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn a_block_has_exactly_the_capacity_asked_for() {
        let memory = SampleMemory::with_budget(100 * S);
        let a = memory.alloc(15).unwrap();
        assert_eq!(a.capacity(), 15);
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn over_budget_is_refused_and_changes_nothing() {
        let memory = SampleMemory::with_budget(50 * S);
        let _a = memory.alloc(40).unwrap();
        let err = memory.alloc(20).unwrap_err();
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
    fn exactly_the_budget_is_allowed() {
        let memory = SampleMemory::with_budget(40 * S);
        let _a = memory.alloc(40).unwrap();
        assert_eq!(memory.in_use(), 40 * S);
        assert!(memory.alloc(1).is_err());
    }

    #[test]
    fn a_count_whose_bytes_overflow_is_refused_as_unaffordable() {
        let memory = SampleMemory::new();
        let err = memory.alloc(usize::MAX).unwrap_err();
        assert_eq!(err.requested, usize::MAX);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn a_block_outlives_the_handle_that_made_it() {
        let memory = SampleMemory::with_budget(100 * S);
        let a = memory.alloc(10).unwrap();
        drop(memory);
        drop(a); // repays into the memory it still holds; must not panic
    }

    #[test]
    fn a_block_reads_as_the_slice_it_holds() {
        let memory = SampleMemory::with_budget(100 * S);
        let mut a = memory.alloc(4).unwrap();
        a.extend([0.25, 0.5, 0.75]);
        assert_eq!(a.len(), 3);
        assert_eq!(a.capacity(), 4);
        assert_eq!(a.room(), 1);
        assert_eq!(&a[..], &[0.25, 0.5, 0.75]);
        assert_eq!(a.iter().sum::<Pcm>(), 1.5);
    }

    #[test]
    fn push_fills_to_capacity_and_then_refuses() {
        let memory = SampleMemory::with_budget(100 * S);
        let mut a = memory.alloc(2).unwrap();
        assert!(a.push(0.1));
        assert!(a.push(0.2));
        assert!(!a.push(0.3));
        assert_eq!(&a[..], &[0.1, 0.2]);
        assert_eq!(a.capacity(), 2);
    }

    #[test]
    fn extend_takes_what_fits_and_drops_the_rest() {
        let memory = SampleMemory::with_budget(100 * S);
        let mut a = memory.alloc(3).unwrap();
        assert_eq!(a.extend([0.1, 0.2]), 2);
        assert_eq!(a.extend([0.3, 0.4, 0.5]), 1);
        assert_eq!(a.extend([0.6]), 0);
        assert_eq!(&a[..], &[0.1, 0.2, 0.3]);
        assert_eq!(a.capacity(), 3);
        assert_eq!(memory.in_use(), 3 * S);
    }

    #[test]
    fn the_filled_part_can_be_written_in_place_and_cut_back() {
        let memory = SampleMemory::with_budget(100 * S);
        let mut a = memory.alloc(4).unwrap();
        // The shape a library fill takes: zero to capacity, lend the
        // slice, keep what was written.
        a.extend(std::iter::repeat_n(0.0, a.capacity()));
        let out: &mut [Pcm] = &mut a;
        out[0] = 0.5;
        out[1] = -0.5;
        a.truncate(2);
        assert_eq!(&a[..], &[0.5, -0.5]);
        assert_eq!(a.capacity(), 4);
        assert_eq!(a.room(), 2);
        assert_eq!(memory.in_use(), 4 * S);
    }

    #[test]
    fn truncate_never_lengthens() {
        let memory = SampleMemory::with_budget(100 * S);
        let mut a = memory.alloc(4).unwrap();
        a.extend([0.1, 0.2]);
        a.truncate(3);
        assert_eq!(a.len(), 2);
    }
}
