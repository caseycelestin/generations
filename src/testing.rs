//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::config::Pcm;
use crate::memory::Ledger;

/// A ledger that refuses nothing, for tests not about the budget.
pub(crate) fn unlimited() -> Arc<Ledger> {
    Ledger::new(usize::MAX)
}

/// `samples` as a buffer of `channels` channels, paid for on a throwaway
/// ledger, for tests that need audio and are not about the budget.
pub(crate) fn audio(channels: u16, samples: &[Pcm]) -> AudioBuffer {
    let mut block = unlimited().alloc(samples.len()).unwrap();
    block.extend(samples.iter().copied());
    AudioBuffer::new(channels, block).unwrap()
}
