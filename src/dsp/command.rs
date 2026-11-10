//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; the engine builds and sends them.
//!
//! The renderer holds one mixer per kind of source, and a command is for
//! exactly one of them, so the enum says which first and what second: a
//! [`SampleCommand`] goes to the sample mixer, an [`InputCommand`] to the
//! input mixer. The renderer only routes; each mixer applies its own.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::Pcm;
use crate::dsp::math::Sides;

/// Identity of one sound: one instance of something heard, a sample
/// played once or an input while it is in the renderer. Minted by the
/// engine when it is started.
///
/// Never reused, so a stop that arrives after its sound was stolen or
/// finished names nothing and does nothing. Minted in order, so the
/// lowest id among the sounds is the oldest. One id names a sample or
/// an input, never both, so a command meant for one mixer cannot land
/// on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoundId(pub u64);

/// Where a resident buffer lives, as a number.
///
/// The render side never learns a sample id, and a raw pointer cannot
/// cross to another thread, so a buffer is named to the renderer by its
/// address. Two live `Arc<AudioBuffer>`s never share one, and the engine
/// sends an eviction before it lets go of the buffer, so the address is
/// still that buffer's when the command is applied: any later buffer at
/// the same address is started by a `Play` that is behind it on the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferAddr(usize);

impl BufferAddr {
    /// The address of the buffer `audio` points at.
    pub fn of(audio: &Arc<AudioBuffer>) -> Self {
        Self(Arc::as_ptr(audio) as usize)
    }
}

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
