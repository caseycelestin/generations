//! The machine's memory budget for audio, and the only source of
//! audio-length memory.
//!
//! [`Ledger`] holds one figure, the budget, and hands out [`Block`]s
//! against it. A block is a fixed-capacity buffer of samples that was paid
//! for when it was made and pays itself back when it is dropped. There is
//! no other way in the crate to get a buffer whose size scales with audio
//! length, so every such byte is checked against the budget before it
//! exists, and release cannot be forgotten or counted twice.
//!
//! The project makes the ledger from the budget the caller gives it and
//! lends it to decode, convert and storage reads. Who may allocate is who
//! holds a `Ledger`; the render side never does. A block holds its own
//! handle to the ledger, but only to repay it: that field is private, so
//! holding a block grants nothing.

use std::mem::size_of;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

use crate::config::Pcm;

/// The memory budget for audio and how much of it is spoken for.
///
/// Made by the project. A handle: clones share the one figure, so every
/// block it has made repays into it on drop, even after the project is
/// gone.
#[derive(Debug, Clone)]
pub(crate) struct Ledger(Arc<Shared>);

/// What a ledger's clones share: the budget and the bytes in use.
#[derive(Debug)]
struct Shared {
    /// The bytes the ledger was made with.
    budget: usize,
    /// Bytes held by live blocks. It protects nothing but itself, so
    /// relaxed ordering is enough.
    in_use: AtomicUsize,
}

impl Ledger {
    /// A ledger of `budget` bytes, none of them in use.
    pub fn new(budget: usize) -> Self {
        Self(Arc::new(Shared {
            budget,
            in_use: AtomicUsize::new(0),
        }))
    }

    /// A block with room for `samples` samples, paid for here.
    ///
    /// Refused, with nothing allocated and nothing changed, if the bytes
    /// would put the ledger over its budget. Reaching exactly the budget
    /// is allowed. A sample count whose bytes cannot be counted on this
    /// machine is refused the same way: what cannot be counted cannot be
    /// afforded.
    ///
    /// The block comes back empty, with exactly the capacity asked for.
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
            ledger: self.clone(),
        })
    }

    /// Bytes held by live blocks right now.
    #[cfg(test)] // read by nothing but tests until a panel asks
    pub fn in_use(&self) -> usize {
        self.0.in_use.load(Ordering::Relaxed)
    }

    /// Gives `bytes` back. Called by a block as it drops; nothing else
    /// has a reason to.
    fn release(&self, bytes: usize) {
        self.0.in_use.fetch_sub(bytes, Ordering::Relaxed);
    }
}

/// The bytes asked for would put the audio in memory over its budget.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{requested} bytes would put audio memory at {} of a {budget} byte budget",
    in_use.saturating_add(*requested)
)]
pub struct OverBudgetError {
    /// Bytes the refused allocation would have cost.
    pub requested: usize,
    /// Bytes in use at the time.
    pub in_use: usize,
    /// The budget the ledger was made with.
    pub budget: usize,
}

/// A fixed-capacity buffer of samples, paid for against a [`Ledger`]:
/// the one shape audio-length memory takes.
///
/// Capacity is set when the block is made and never changes: it cannot
/// grow by doubling, so what was paid for is what is held. Length is how
/// much of it is in use; a block made for a recorder starts empty and
/// fills, a block made for a decode is filled at once. Payment is always
/// for the capacity.
///
/// Dropping the block gives `capacity * size_of::<Pcm>()` bytes back.
/// Wrapped in an `Arc`, it is at once the data, the read ticket and the
/// receipt: the bytes return when the last reader lets go.
#[derive(Debug)]
pub struct Block {
    /// The samples. Never reallocated after construction.
    data: Vec<Pcm>,
    /// Where the capacity was charged, to repay it on drop.
    ledger: Ledger,
}

impl Block {
    /// Samples the block has room for. Fixed for its life; what was paid
    /// for.
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Samples the block still has room for.
    pub fn room(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Appends one sample, or drops it if the block is full. Returns
    /// whether it went in.
    pub fn push(&mut self, sample: Pcm) -> bool {
        if self.room() == 0 {
            return false;
        }
        self.data.push(sample);
        true
    }

    /// Appends samples until the block is full or `samples` runs out,
    /// whichever comes first, and returns how many went in. Samples past
    /// the room are dropped, never stored.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = Pcm>) -> usize {
        let before = self.len();
        let room = self.room();
        self.data.extend(samples.into_iter().take(room));
        debug_assert_eq!(self.data.capacity(), self.capacity(), "a block grew");
        self.len() - before
    }

    /// Shortens the block to `len` samples, dropping the rest. Does
    /// nothing if it already holds that few. The capacity is unchanged:
    /// the room comes back, the bytes stay paid for.
    pub fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }
}

/// A block reads as the slice of samples it holds so far. Everything a
/// slice can do, a block can: length, indexing, iteration, chunking.
impl Deref for Block {
    type Target = [Pcm];

    fn deref(&self) -> &[Pcm] {
        &self.data
    }
}

/// The filled part can be written in place. The length cannot change
/// through this view, so a library handed `&mut block[..]` can fill what
/// is there and nothing more.
impl DerefMut for Block {
    fn deref_mut(&mut self) -> &mut [Pcm] {
        &mut self.data
    }
}

impl Drop for Block {
    /// Gives the capacity back. Capacity, not length: payment was for
    /// what is held, whether or not it was filled.
    fn drop(&mut self) {
        self.ledger.release(self.data.capacity() * size_of::<Pcm>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes of one sample.
    const S: usize = size_of::<Pcm>();

    #[test]
    fn a_new_ledger_has_nothing_in_use() {
        let ledger = Ledger::new(100);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn an_alloc_of_exactly_what_is_left_succeeds() {
        let ledger = Ledger::new(100 * S);
        let _a = ledger.alloc(30).unwrap();
        let _b = ledger.alloc(70).unwrap();
        assert_eq!(ledger.in_use(), 100 * S);
    }

    #[test]
    fn a_block_counts_until_it_is_dropped() {
        let ledger = Ledger::new(100 * S);
        let a = ledger.alloc(15).unwrap();
        let b = ledger.alloc(5).unwrap();
        assert_eq!(ledger.in_use(), 20 * S);
        drop(a);
        assert_eq!(ledger.in_use(), 5 * S);
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_block_has_exactly_the_capacity_asked_for() {
        let ledger = Ledger::new(100 * S);
        let a = ledger.alloc(15).unwrap();
        assert_eq!(a.capacity(), 15);
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn over_budget_is_refused_and_changes_nothing() {
        let ledger = Ledger::new(50 * S);
        let _a = ledger.alloc(40).unwrap();
        let err = ledger.alloc(20).unwrap_err();
        assert_eq!(
            err,
            OverBudgetError {
                requested: 20 * S,
                in_use: 40 * S,
                budget: 50 * S,
            }
        );
        assert_eq!(ledger.in_use(), 40 * S);
    }

    #[test]
    fn exactly_the_budget_is_allowed() {
        let ledger = Ledger::new(40 * S);
        let _a = ledger.alloc(40).unwrap();
        assert_eq!(ledger.in_use(), 40 * S);
        assert!(ledger.alloc(1).is_err());
    }

    #[test]
    fn a_count_whose_bytes_overflow_is_refused_as_unaffordable() {
        let ledger = Ledger::new(usize::MAX);
        let err = ledger.alloc(usize::MAX).unwrap_err();
        assert_eq!(err.requested, usize::MAX);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_block_outlives_the_handle_that_made_it() {
        let ledger = Ledger::new(100 * S);
        let a = ledger.alloc(10).unwrap();
        drop(ledger);
        drop(a); // repays into the ledger it still holds; must not panic
    }

    #[test]
    fn a_block_reads_as_the_slice_it_holds() {
        let ledger = Ledger::new(100 * S);
        let mut a = ledger.alloc(4).unwrap();
        a.extend([0.25, 0.5, 0.75]);
        assert_eq!(a.len(), 3);
        assert_eq!(a.capacity(), 4);
        assert_eq!(a.room(), 1);
        assert_eq!(&a[..], &[0.25, 0.5, 0.75]);
        assert_eq!(a.iter().sum::<Pcm>(), 1.5);
    }

    #[test]
    fn push_fills_to_capacity_and_then_refuses() {
        let ledger = Ledger::new(100 * S);
        let mut a = ledger.alloc(2).unwrap();
        assert!(a.push(0.1));
        assert!(a.push(0.2));
        assert!(!a.push(0.3));
        assert_eq!(&a[..], &[0.1, 0.2]);
        assert_eq!(a.capacity(), 2);
    }

    #[test]
    fn extend_takes_what_fits_and_drops_the_rest() {
        let ledger = Ledger::new(100 * S);
        let mut a = ledger.alloc(3).unwrap();
        assert_eq!(a.extend([0.1, 0.2]), 2);
        assert_eq!(a.extend([0.3, 0.4, 0.5]), 1);
        assert_eq!(a.extend([0.6]), 0);
        assert_eq!(&a[..], &[0.1, 0.2, 0.3]);
        assert_eq!(a.capacity(), 3);
        assert_eq!(ledger.in_use(), 3 * S);
    }

    #[test]
    fn the_filled_part_can_be_written_in_place_and_cut_back() {
        let ledger = Ledger::new(100 * S);
        let mut a = ledger.alloc(4).unwrap();
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
        assert_eq!(ledger.in_use(), 4 * S);
    }

    #[test]
    fn truncate_never_lengthens() {
        let ledger = Ledger::new(100 * S);
        let mut a = ledger.alloc(4).unwrap();
        a.extend([0.1, 0.2]);
        a.truncate(3);
        assert_eq!(a.len(), 2);
    }
}
