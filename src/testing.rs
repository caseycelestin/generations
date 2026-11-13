//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::config::Pcm;
use crate::memory::SampleMemory;

/// `samples` as a buffer of `layout`, paid for on throwaway memory, for
/// tests that need audio and are not about the budget.
pub(crate) fn audio(layout: ChannelLayout, samples: &[Pcm]) -> AudioBuffer {
    let mut block = SampleMemory::new().alloc(samples.len()).unwrap();
    block.extend(samples.iter().copied());
    AudioBuffer::new(layout, block).unwrap()
}
