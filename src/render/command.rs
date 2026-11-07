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
/// Each command is keyed a different way: `Play` and `Stop` by voice,
/// `StopAll` by nothing, `Evict` by buffer.
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
    /// Stop every voice playing the buffer at `addr`, so the engine can
    /// let go of the last reference to it.
    Evict(BufferAddr),
}
