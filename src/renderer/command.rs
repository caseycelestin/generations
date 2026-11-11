//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; its handle builds and sends them.
//!
//! The renderer holds one mixer per kind of source, and a command is for
//! exactly one of them, so the enum says which first and what second: a
//! [`SampleCommand`] goes to the sample mixer, an [`InputCommand`] to the
//! input mixer. A [`SoundId`] names one sound in one mixer or the other,
//! a sample playing or one source of an input, never both, so a command
//! meant for one side cannot land on the other.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::dsp::{BufferAddress, InputReader, SoundId};

/// What the handle can ask of the renderer: a command for one of its
/// mixers.
///
/// An `Add` carries a whole `InputReader`, which is wide; every command
/// is a slot of that size on a ring allocated once at open, so the size
/// is paid there and nowhere else.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
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
    /// Stop every sound playing the buffer at `addr`, so the control side
    /// can let go of the last reference to it.
    Evict(BufferAddress),
}

/// What can be asked of the input mixer.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum InputCommand {
    /// Add one source of an input, the sound `id`, whose ring `reader` is
    /// the render side of, not yet heard. The ring was made for this source
    /// and this is its reading end's only crossing.
    Add { id: SoundId, reader: InputReader },
    /// Remove the input sound `id`, dropping its reader.
    Remove(SoundId),
    /// Hear the input sound `id` through the output.
    Monitor(SoundId),
    /// Stop hearing the input sound `id`; it stays an input.
    Unmonitor(SoundId),
}
