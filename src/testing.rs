//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::config::Pcm;
use crate::project::ledger::Ledger;
use crate::render::Renderer;
use crate::render::dsp::math::add;
use crate::render::dsp::{Recorder, Source};

/// Adds one period of `source` into `out`, an interleaved stereo slice,
/// the period being as many frames as `out` holds: what the renderer
/// does with a source, for tests of one.
pub(crate) fn hear(source: &mut impl Source, out: &mut [Pcm]) {
    add(out, source.frames(out.len() / Renderer::CHANNELS));
}

/// A ledger that refuses nothing, for tests not about the budget.
pub(crate) fn unlimited() -> Ledger {
    Ledger::new(usize::MAX)
}

/// `samples` as a buffer of `layout`, paid for on a throwaway ledger, for
/// tests that need audio and are not about the budget.
pub(crate) fn audio(layout: ChannelLayout, samples: &[Pcm]) -> AudioBuffer {
    let mut block = unlimited().alloc(samples.len()).unwrap();
    block.extend(samples.iter().copied());
    AudioBuffer::new(layout, block).unwrap()
}

/// An empty stereo buffer with room for `frames` frames, paid for on a
/// throwaway ledger, for a take in tests not about the budget.
pub(crate) fn take_buffer(frames: usize) -> AudioBuffer {
    let block = unlimited().alloc(frames * Renderer::CHANNELS).unwrap();
    AudioBuffer::new(ChannelLayout::Stereo, block).unwrap()
}

/// A recorder starting a take in `take_buffer(frames)`.
pub(crate) fn recorder(frames: usize) -> Recorder {
    Recorder::new(take_buffer(frames))
}
