//! The audio thread: turns commands and audio into periods of output.

mod math;
mod voices;

use std::num::NonZeroUsize;

use rtrb::{Consumer, Producer};

use crate::config::Pcm;
use crate::engine::protocol::{Command, Return};
use math::hard_clip;
use voices::Voices;

/// Channels in every frame the renderer makes: always stereo. A mono
/// source is heard on both sides.
pub(crate) const CHANNELS: usize = 2;

/// One output frame: a sample per side.
pub(crate) type Frame = [Pcm; CHANNELS];

#[derive(Debug)]
pub struct Renderer {
    from_control: Consumer<Command>,
    to_control: Producer<Return>,
    voices: Voices,
}

impl Renderer {
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
        }
    }

    /// Renders one period into `out`, interleaved stereo, replacing what
    /// it held. Silence until something can play.
    pub fn render(&mut self, out: &mut [Pcm]) {
        self.apply_commands();
        out.fill(0.0);
        hard_clip(out);
    }

    /// Applies every command waiting, oldest first.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.from_control.pop() {
            match command {
                // Dropped until voices have somewhere to live.
                Command::Voice(_) => {}
            }
        }
    }
}
