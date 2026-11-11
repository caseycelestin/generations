//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::config::Pcm;
use crate::memory::Ledger;
use crate::render::Renderer;
use crate::render::dsp::Source;
use crate::render::dsp::math::add;

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
