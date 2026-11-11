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
//! The caller makes the ledger, because the budget is the machine's to
//! know, and hands it to whatever allocates: today the project, which
//! lends it to decode, convert and storage reads. One ledger per machine,
//! shared by everything that holds audio in RAM, so the figure is one
//! figure.
//! Who may allocate is decided by who holds a `Ledger`. The control side
//! does.
//! The render side never does, and so cannot allocate even by mistake;
//! what it works on arrives already allocated, by command. A block holds
//! its own handle to the ledger it came from, but only to repay it: that
//! field is private to this module, so holding a block grants nothing.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

pub mod block;

pub use block::Block;

/// The memory budget for audio and how much of it is spoken for.
///
/// Made by the caller with the machine's figure. A handle: cloning it
/// shares the one figure, so everything that allocates from it, the
/// project and whatever it lends it to, and every block it has made,
/// which repays into it on drop, hold the same ledger. One per machine.
#[derive(Debug, Clone)]
pub struct Ledger(Arc<Shared>);

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

    /// A block with room for `samples` samples of `T`, paid for here.
    ///
    /// Refused, with nothing allocated and nothing changed, if the bytes
    /// would put the ledger over its budget. Reaching exactly the budget
    /// is allowed. A sample count whose bytes cannot be counted on this
    /// machine is refused the same way: what cannot be counted cannot be
    /// afforded.
    ///
    /// The block comes back empty, with exactly the capacity asked for.
    pub fn alloc<T>(&self, samples: usize) -> Result<Block<T>, OverBudget> {
        let shared = &*self.0;
        let over = |requested, in_use| OverBudget {
            requested,
            in_use,
            budget: shared.budget,
        };
        let bytes = samples
            .checked_mul(size_of::<T>())
            .ok_or_else(|| over(usize::MAX, shared.in_use.load(Ordering::Relaxed)))?;
        shared
            .in_use
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                in_use
                    .checked_add(bytes)
                    .filter(|&total| total <= shared.budget)
            })
            .map_err(|in_use| over(bytes, in_use))?;
        Ok(Block::new(Vec::with_capacity(samples), self.clone()))
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
pub struct OverBudget {
    /// Bytes the refused allocation would have cost.
    pub requested: usize,
    /// Bytes in use at the time.
    pub in_use: usize,
    /// The budget the ledger was made with.
    pub budget: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_ledger_has_nothing_in_use() {
        let ledger = Ledger::new(100);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn an_alloc_of_exactly_what_is_left_succeeds() {
        let ledger = Ledger::new(100);
        let _a: Block<u8> = ledger.alloc(30).unwrap();
        let _b: Block<u8> = ledger.alloc(70).unwrap();
        assert_eq!(ledger.in_use(), 100);
    }

    #[test]
    fn a_block_counts_until_it_is_dropped() {
        let ledger = Ledger::new(100);
        let a: Block<i16> = ledger.alloc(15).unwrap(); // 30 bytes
        let b: Block<f32> = ledger.alloc(5).unwrap(); // 20 bytes
        assert_eq!(ledger.in_use(), 50);
        drop(a);
        assert_eq!(ledger.in_use(), 20);
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_block_has_exactly_the_capacity_asked_for() {
        let ledger = Ledger::new(100);
        let a: Block<i16> = ledger.alloc(15).unwrap();
        assert_eq!(a.capacity(), 15);
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn over_budget_is_refused_and_changes_nothing() {
        let ledger = Ledger::new(50);
        let _a: Block<u8> = ledger.alloc(40).unwrap();
        let err = ledger.alloc::<u8>(20).unwrap_err();
        assert_eq!(
            err,
            OverBudget {
                requested: 20,
                in_use: 40,
                budget: 50,
            }
        );
        assert_eq!(ledger.in_use(), 40);
    }

    #[test]
    fn exactly_the_budget_is_allowed() {
        let ledger = Ledger::new(40);
        let _a: Block<u8> = ledger.alloc(40).unwrap();
        assert_eq!(ledger.in_use(), 40);
        assert!(ledger.alloc::<u8>(1).is_err());
    }

    #[test]
    fn a_count_whose_bytes_overflow_is_refused_as_unaffordable() {
        let ledger = Ledger::new(usize::MAX);
        let err = ledger.alloc::<f32>(usize::MAX).unwrap_err();
        assert_eq!(err.requested, usize::MAX);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_block_outlives_the_handle_that_made_it() {
        let ledger = Ledger::new(100);
        let a: Block<u8> = ledger.alloc(10).unwrap();
        drop(ledger);
        drop(a); // repays into the ledger it still holds; must not panic
    }
}
