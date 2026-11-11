//! Frames written into a block, first frame to last, until it is full.
//!
//! A [`Recorder`] is the other end of a [`Source`](crate::render::dsp::Source):
//! a source gives frames, a recorder keeps them. It holds one block, paid
//! for on the control side when the recorder is made, and appends each
//! period's frames to it. A full recorder keeps nothing more; being full
//! is not an error.
//!
//! One recorder makes many takes. Clearing it empties the block and keeps
//! what was paid for, so starting another take allocates nothing.

use crate::config::Pcm;
use crate::memory::{Block, Ledger, OverBudgetError};
use crate::render::Renderer;
use crate::render::dsp::math::Frame;

/// A block of stereo frames, filled from the first frame on.
///
/// The block holds whole frames, from creation and after every write;
/// that is the one thing this type guards.
#[derive(Debug)]
pub struct Recorder {
    block: Block<Pcm>,
}

impl Recorder {
    /// Room for `frames` stereo frames, paid for against `ledger`.
    ///
    /// Refused, with nothing allocated, if the ledger cannot afford it.
    #[cfg_attr(not(test), expect(dead_code, reason = "no control-side sender yet"))]
    pub fn new(ledger: &Ledger, frames: usize) -> Result<Self, OverBudgetError> {
        let samples = frames.saturating_mul(Renderer::CHANNELS);
        Ok(Self {
            block: ledger.alloc(samples)?,
        })
    }

    /// Keeps `frames` after what is already kept, up to the room left.
    /// Frames past the room are dropped.
    pub fn record(&mut self, frames: impl IntoIterator<Item = Frame>) {
        self.block.extend(frames.into_iter().flatten());
    }

    /// Forgets what it kept, so the next frames start a new take. The
    /// room comes back; the block stays paid for.
    #[cfg_attr(not(test), expect(dead_code, reason = "no control-side owner yet"))]
    pub fn clear(&mut self) {
        self.block.truncate(0);
    }

    /// Whether it has no room for another frame.
    pub fn full(&self) -> bool {
        self.block.room() == 0
    }

    /// The frames kept so far, interleaved.
    #[cfg(test)] // read by nothing but tests until a take is handed back
    pub fn recorded(&self) -> &[Pcm] {
        &self.block
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::unlimited;

    #[test]
    fn a_new_recorder_is_empty() {
        let recorder = Recorder::new(&unlimited(), 4).unwrap();
        assert!(recorder.recorded().is_empty());
        assert!(!recorder.full());
    }

    #[test]
    fn frames_are_kept_in_order_across_periods() {
        let mut recorder = Recorder::new(&unlimited(), 4).unwrap();
        recorder.record([[0.1, 0.2], [0.3, 0.4]]);
        recorder.record([[0.5, 0.6]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert!(!recorder.full());
    }

    #[test]
    fn frames_past_the_room_are_dropped() {
        let mut recorder = Recorder::new(&unlimited(), 2).unwrap();
        recorder.record([[0.1, 0.2], [0.3, 0.4], [0.5, 0.6]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4]);
        assert!(recorder.full());
        recorder.record([[0.7, 0.8]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn a_cleared_recorder_starts_a_new_take_and_keeps_its_payment() {
        let ledger = Ledger::new(usize::MAX);
        let mut recorder = Recorder::new(&ledger, 1).unwrap();
        recorder.record([[0.1, 0.2]]);
        assert!(recorder.full());
        let paid = ledger.in_use();
        recorder.clear();
        assert!(recorder.recorded().is_empty());
        assert!(!recorder.full());
        recorder.record([[0.3, 0.4]]);
        assert_eq!(recorder.recorded(), &[0.3, 0.4]);
        assert_eq!(ledger.in_use(), paid);
    }

    #[test]
    fn a_recorder_the_ledger_cannot_afford_is_refused() {
        let ledger = Ledger::new(4 * size_of::<Pcm>() - 1);
        assert!(Recorder::new(&ledger, 2).is_err());
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn the_whole_block_is_paid_for_up_front() {
        let ledger = Ledger::new(usize::MAX);
        let _recorder = Recorder::new(&ledger, 3).unwrap();
        assert_eq!(ledger.in_use(), 3 * Renderer::CHANNELS * size_of::<Pcm>());
    }
}
