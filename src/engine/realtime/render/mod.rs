//! The audio thread: turns commands and audio into periods of output.

mod math;
mod voices;

use std::num::NonZeroUsize;

use rtrb::{Consumer, Producer};

use crate::config::Pcm;
use crate::engine::protocol::{Command, Return};
use math::{add, hard_clip};
use voices::Voices;

/// One output frame: a sample per side.
pub(crate) type Frame = [Pcm; Renderer::CHANNELS];

#[derive(Debug)]
pub struct Renderer {
    from_control: Consumer<Command>,
    to_control: Producer<Return>,
    voices: Voices,
    /// One sound's frames for the block, before they join the mix.
    scratch: Box<[Frame]>,
}

impl Renderer {
    /// Always stereo.
    pub(crate) const CHANNELS: usize = 2;

    /// A period of any length is mixed in blocks this long, so scratch
    /// never grows.
    pub(crate) const BLOCK_FRAMES: usize = 64;

    /// Allocates everything the renderer holds for its life, here, on the
    /// control side.
    pub(crate) fn new(
        from_control: Consumer<Command>,
        to_control: Producer<Return>,
        voices: NonZeroUsize,
    ) -> Self {
        Self {
            from_control,
            to_control,
            voices: Voices::new(voices),
            scratch: vec![[0.0; Self::CHANNELS]; Self::BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// Renders one period into `out`, interleaved stereo, replacing what
    /// it held: every playing voice, summed and clipped.
    pub fn render(&mut self, out: &mut [Pcm]) {
        debug_assert_eq!(out.len() % Self::CHANNELS, 0, "output is not whole frames");
        self.apply_commands();
        let (frames, _) = out.as_chunks_mut::<{ Self::CHANNELS }>();
        for block in frames.chunks_mut(Self::BLOCK_FRAMES) {
            block.fill([0.0; Self::CHANNELS]);
            let scratch = &mut self.scratch[..block.len()];
            for voice in self.voices.iter_mut() {
                voice.render(scratch);
                add(block, scratch);
            }
        }
        hard_clip(out);
    }

    /// Applies every command waiting, oldest first.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.from_control.pop() {
            match command {
                Command::Voice(command) => self.voices.apply(command, &mut self.to_control),
            }
        }
    }
}
