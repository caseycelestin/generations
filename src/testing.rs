//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use crate::audio::{AudioBuffer, AudioMemory};
use crate::config::Pcm;
use crate::layout::ChannelLayout;

/// `samples` as a buffer of `layout`, paid for on throwaway memory, for
/// tests that need audio and are not about the budget.
pub(crate) fn buffer(layout: ChannelLayout, samples: &[Pcm]) -> AudioBuffer {
    let frames = samples.len() / layout.count();
    let mut audio = AudioMemory::new().alloc_buffer(layout, frames).unwrap();
    audio.extend(samples.iter().copied());
    assert_eq!(audio.samples(), samples, "whole frames of `layout`");
    audio
}
