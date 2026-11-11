//! A fixed-capacity buffer of samples, paid for against a [`Ledger`].
//!
//! The one shape audio-length memory takes. A block never grows: every
//! way in appends up to the capacity and no further, so what was paid for
//! at `alloc` is what is ever held, and no write can reallocate, on the
//! audio thread or anywhere else.

use super::Ledger;
use std::mem::size_of;
use std::ops::{Deref, DerefMut};

/// A fixed-capacity buffer of `T` samples, paid for against a [`Ledger`].
///
/// Capacity is set when the block is made and never changes: it cannot
/// grow by doubling, so what was paid for is what is held. Length is how
/// much of it is in use; a block made for a recorder starts empty and
/// fills, a block made for a decode is filled at once. Payment is always
/// for the capacity.
///
/// Dropping the block gives `capacity * size_of::<T>()` bytes back.
/// Wrapped in an `Arc`, it is at once the data, the read ticket and the
/// receipt: the bytes return when the last reader lets go.
#[derive(Debug)]
pub struct Block<T> {
    /// The samples. Never reallocated after construction.
    data: Vec<T>,
    /// Where the capacity was charged, to repay it on drop.
    ledger: Ledger,
}

impl<T> Block<T> {
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
    pub fn push(&mut self, sample: T) -> bool {
        if self.room() == 0 {
            return false;
        }
        self.data.push(sample);
        true
    }

    /// Appends samples until the block is full or `samples` runs out,
    /// whichever comes first, and returns how many went in. Samples past
    /// the room are dropped, never stored.
    pub fn extend(&mut self, samples: impl IntoIterator<Item = T>) -> usize {
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

    /// Writes every sample of the capacity once, then empties the block,
    /// so the OS backs every page of it now. A fresh block is reserved,
    /// not backed: the first write to each page stops the thread while
    /// the kernel finds one. Call it on the control side for a block that
    /// will be filled on the audio thread, so that thread never waits.
    pub fn prefault(&mut self)
    where
        T: Default + Clone,
    {
        self.truncate(0);
        self.extend(std::iter::repeat_n(T::default(), self.capacity()));
        // The writes are never read; keep the optimiser from dropping them.
        std::hint::black_box(&self.data[..]);
        self.truncate(0);
    }

    /// Wraps a buffer already paid for against `ledger`. Only `alloc`
    /// calls this; it is what has done the paying.
    pub(super) fn new(data: Vec<T>, ledger: Ledger) -> Self {
        Self { data, ledger }
    }
}

/// A block reads as the slice of samples it holds so far. Everything a
/// slice can do, a block can: length, indexing, iteration, chunking.
impl<T> Deref for Block<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.data
    }
}

/// The filled part can be written in place. The length cannot change
/// through this view, so a library handed `&mut block[..]` can fill what
/// is there and nothing more.
impl<T> DerefMut for Block<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.data
    }
}

impl<T> Drop for Block<T> {
    /// Gives the capacity back. Capacity, not length: payment was for
    /// what is held, whether or not it was filled.
    fn drop(&mut self) {
        self.ledger.release(self.data.capacity() * size_of::<T>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefaulted_block_is_empty_with_its_capacity_and_payment() {
        let ledger = Ledger::new(usize::MAX);
        let mut block: Block<f32> = ledger.alloc(1000).unwrap();
        block.prefault();
        assert!(block.is_empty());
        assert_eq!(block.capacity(), 1000);
        assert_eq!(ledger.in_use(), 1000 * size_of::<f32>());
    }

    #[test]
    fn a_block_reads_as_the_slice_it_holds() {
        let ledger = Ledger::new(100);
        let mut a: Block<i16> = ledger.alloc(4).unwrap();
        a.extend([1, 2, 3]);
        assert_eq!(a.len(), 3);
        assert_eq!(a.capacity(), 4);
        assert_eq!(a.room(), 1);
        assert_eq!(&a[..], &[1, 2, 3]);
        assert_eq!(a.iter().sum::<i16>(), 6);
    }

    #[test]
    fn push_fills_to_capacity_and_then_refuses() {
        let ledger = Ledger::new(100);
        let mut a: Block<u8> = ledger.alloc(2).unwrap();
        assert!(a.push(1));
        assert!(a.push(2));
        assert!(!a.push(3));
        assert_eq!(&a[..], &[1, 2]);
        assert_eq!(a.capacity(), 2);
    }

    #[test]
    fn extend_takes_what_fits_and_drops_the_rest() {
        let ledger = Ledger::new(100);
        let mut a: Block<u8> = ledger.alloc(3).unwrap();
        assert_eq!(a.extend([1, 2]), 2);
        assert_eq!(a.extend([3, 4, 5]), 1);
        assert_eq!(a.extend([6]), 0);
        assert_eq!(&a[..], &[1, 2, 3]);
        assert_eq!(a.capacity(), 3);
        assert_eq!(ledger.in_use(), 3);
    }

    #[test]
    fn the_filled_part_can_be_written_in_place_and_cut_back() {
        let ledger = Ledger::new(100);
        let mut a: Block<f32> = ledger.alloc(4).unwrap();
        // The shape a library fill takes: zero to capacity, lend the
        // slice, keep what was written.
        a.extend(std::iter::repeat_n(0.0, a.capacity()));
        let out: &mut [f32] = &mut a;
        out[0] = 0.5;
        out[1] = -0.5;
        a.truncate(2);
        assert_eq!(&a[..], &[0.5, -0.5]);
        assert_eq!(a.capacity(), 4);
        assert_eq!(a.room(), 2);
        assert_eq!(ledger.in_use(), 16);
    }

    #[test]
    fn truncate_never_lengthens() {
        let ledger = Ledger::new(100);
        let mut a: Block<u8> = ledger.alloc(4).unwrap();
        a.extend([1, 2]);
        a.truncate(3);
        assert_eq!(a.len(), 2);
    }
}
