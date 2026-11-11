//! A sound the caller holds: one play of a sample, and what can be done
//! to it while it plays.
//!
//! Playing a [`LoadedSample`] returns a [`Sound`]. The sound's verbs are
//! on the sound: `stop` is `sound.stop()`, not a method elsewhere that
//! takes an id. The id the renderer knows the sound by is inside the handle and
//! never leaves it; the only way to get a `Sound` is to play one, so a
//! `Sound` always names a sound that was started. Dropping the handle
//! changes nothing: the sound plays to its end. It owns nothing; the
//! audio is the [`LoadedSample`]'s and the slot is the mixer's.
//!
//!
//! [`LoadedSample`]: crate::loaded_sample::LoadedSample

use crate::dsp::SoundId;
use crate::engine::EngineError;
use crate::renderer_handle::RendererHandle;

/// One play of a sample. Drop it and the sound plays on to its end;
/// `stop` it to cut it.
#[derive(Debug)]
pub struct Sound {
    id: SoundId,
    renderer: RendererHandle,
}

impl Sound {
    /// The sound `id`, playing on `renderer`.
    pub(crate) fn new(id: SoundId, renderer: RendererHandle) -> Self {
        Self { id, renderer }
    }

    /// The id the renderer knows this sound by.
    #[cfg(test)]
    pub(crate) fn id(&self) -> SoundId {
        self.id
    }

    /// Stops the sound if it is still playing, at the output's next
    /// period. One that has finished or been stolen is left alone, so
    /// stopping twice is harmless. Refused as `CommandsFull` if the ring
    /// has no room.
    pub fn stop(&self) -> Result<(), EngineError> {
        self.renderer.stop(self.id)
    }
}
