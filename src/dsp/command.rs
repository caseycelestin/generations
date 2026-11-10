//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; the engine builds and sends them.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::Pcm;
use crate::dsp::math::Sides;

/// Identity of one sound: one instance of something heard, a sample
/// played once or an input while it is monitored. Minted by the engine
/// when it is started.
///
/// Never reused, so a stop that arrives after its sound was stolen or
/// finished names nothing and does nothing. Minted in order, so the
/// lowest id among the sounds is the oldest.
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

/// What the engine can ask of the renderer.
///
/// Samples and inputs have their own commands: `Play`, `Stop`, `StopAll`
/// and `Evict` touch the sample mixer; `Input`, `RemoveInput`, `Monitor`
/// and `Unmonitor` the input mixer. A sound id names one or the other,
/// never both, so a stop meant for one side cannot land on the other.
#[derive(Debug)]
pub enum Command {
    /// Start `audio` from its first frame as the sound `id`.
    Play {
        id: SoundId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop the sample `id` if it is still playing.
    Stop(SoundId),
    /// Stop every sample. Inputs keep being heard.
    StopAll,
    /// Stop every sound playing the buffer at `addr`, so the engine can
    /// let go of the last reference to it.
    Evict(BufferAddr),
    /// Add the input whose ring `reader` is the end of, as `id`, with
    /// the capture channels `sides` on the output's sides, not yet heard.
    /// The ring was made for this input and this is its reading end's
    /// only crossing.
    Input {
        id: SoundId,
        reader: Consumer<Pcm>,
        sides: Sides,
    },
    /// Remove the input `id`, dropping its reader.
    RemoveInput(SoundId),
    /// Hear the input `id` through the output.
    Monitor(SoundId),
    /// Stop hearing the input `id`; it stays an input.
    Unmonitor(SoundId),
}
