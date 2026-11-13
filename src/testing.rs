//! Scaffolding shared by tests across the crate.

use crate::audio::{AudioBuffer, AudioMemory};
use crate::config::Pcm;
use crate::layout::ChannelLayout;

/// `samples` as a buffer of `layout`, paid for on throwaway memory, for
/// tests that need audio and are not about the budget.
pub(crate) fn buffer(layout: ChannelLayout, samples: &[Pcm]) -> AudioBuffer {
    let frames = samples.len() / layout.count();
    let mut audio = AudioMemory::new().alloc_buffer(layout, frames).unwrap();
    let filled = audio.extend(samples.iter().copied());
    assert_eq!(
        filled * layout.count(),
        samples.len(),
        "whole frames of `layout`"
    );
    audio
}
