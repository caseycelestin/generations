//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; the engine builds and sends them.

use std::sync::Arc;

use crate::audio::AudioBuffer;

/// Identity of one started sound, minted by the engine.
///
/// Never reused, so a stop that arrives after its voice was stolen or
/// finished names nothing and does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u64);

/// What the engine can ask of the renderer.
#[derive(Debug)]
pub enum Command {
    /// Start `audio` from its first frame as voice `id`.
    Play {
        id: VoiceId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop voice `id` if it is still playing.
    Stop(VoiceId),
    /// Stop every voice.
    StopAll,
}
