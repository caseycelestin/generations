//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; the engine builds and sends them.
//!
//! The renderer holds one mixer per kind of source, and a command is for
//! exactly one of them, so the enum says which first and what second: a
//! [`SampleCommand`] goes to the sample mixer, an [`InputCommand`] to the
//! input mixer. A [`SoundId`] names a sound in one mixer or the other,
//! never both, so a command meant for one cannot land on the other.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::Pcm;
use crate::dsp::math::Sides;
use crate::dsp::{BufferAddr, SoundId};

/// What the engine can ask of the renderer: a command for one of its
/// mixers.
#[derive(Debug)]
pub enum Command {
    /// For the sample mixer.
    Sample(SampleCommand),
    /// For the input mixer.
    Input(InputCommand),
}

impl From<SampleCommand> for Command {
    fn from(command: SampleCommand) -> Self {
        Self::Sample(command)
    }
}

impl From<InputCommand> for Command {
    fn from(command: InputCommand) -> Self {
        Self::Input(command)
    }
}

/// What can be asked of the sample mixer.
#[derive(Debug)]
pub enum SampleCommand {
    /// Start `audio` from its first frame as the sound `id`.
    Play {
        id: SoundId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop the sample `id` if it is still playing.
    Stop(SoundId),
    /// Stop every sample.
    StopAll,
    /// Stop every sound playing the buffer at `addr`, so the engine can
    /// let go of the last reference to it.
    Evict(BufferAddr),
}

/// What can be asked of the input mixer.
#[derive(Debug)]
pub enum InputCommand {
    /// Add the input whose ring `reader` is the end of, as `id`, with
    /// the capture channels `sides` on the output's sides, not yet heard.
    /// The ring was made for this input and this is its reading end's
    /// only crossing.
    Add {
        id: SoundId,
        reader: Consumer<Pcm>,
        sides: Sides,
    },
    /// Remove the input `id`, dropping its reader.
    Remove(SoundId),
    /// Hear the input `id` through the output.
    Monitor(SoundId),
    /// Stop hearing the input `id`; it stays an input.
    Unmonitor(SoundId),
}
