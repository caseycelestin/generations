//! A loaded sample: its audio in RAM, owned by whoever holds the handle.
//!
//! `Engine::load` reads a sample's audio into a block on the ledger and
//! returns it as a [`LoadedSample`]. The handle is the audio's owner.
//! Hold it and the sample is in RAM; drop it and the sample is unloaded:
//! every sound playing it is cut at the output's next render and the
//! block repays the budget once the last of them has let go. There is no
//! list of loaded samples anywhere else and no unload call: what is
//! loaded is what is held, which is how Rust says it, a `File` is open
//! while you hold it.
//!
//! Playing is a verb on the loaded sample, because it needs nothing but
//! the audio, a fresh sound id and the command ring, and the handle
//! carries all three.
//!
//! The project records nothing about what is loaded. What to load when a
//! project is opened is the caller's to remember, until there is a panel
//! with an opinion.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::dsp::BufferAddr;
use crate::engine::EngineError;
use crate::project::SampleId;
use crate::renderer_handle::RendererHandle;
use crate::sound::Sound;

/// A sample's audio in RAM. Owning this is what loaded means.
#[derive(Debug)]
pub struct LoadedSample {
    id: SampleId,
    /// Immutable behind an `Arc` so a sound can hold it while it plays;
    /// the render side only ever sees the clones it was handed.
    audio: Arc<AudioBuffer>,
    renderer: RendererHandle,
}

impl LoadedSample {
    /// The sample `id`, whose audio is `audio`, playing on `renderer`.
    pub(crate) fn new(id: SampleId, audio: AudioBuffer, renderer: RendererHandle) -> Self {
        Self {
            id,
            audio: Arc::new(audio),
            renderer,
        }
    }

    /// Which sample this is.
    pub fn id(&self) -> SampleId {
        self.id
    }

    /// The audio, for tests.
    #[cfg(test)]
    pub(crate) fn audio(&self) -> &Arc<AudioBuffer> {
        &self.audio
    }

    /// Starts playing the sample from its first frame and returns the
    /// sound, which is how it is stopped: [`Sound::stop`].
    ///
    /// Nothing is read from disk here, so a press costs the same every
    /// time. Sound starts at the output's next render. A play never fails
    /// for want of a free slot: the renderer steals the oldest. Refused as
    /// `CommandsFull` if the ring has no room, and then no id is minted.
    pub fn play(&self) -> Result<Sound, EngineError> {
        let id = self.renderer.play(Arc::clone(&self.audio))?;
        Ok(Sound::new(id, self.renderer.clone()))
    }
}

impl Drop for LoadedSample {
    /// Unloads the sample: tells the renderer to stop every sound playing
    /// it, then lets the audio go. The eviction goes out before the
    /// reference does, which is what makes the address a safe key; see
    /// `BufferAddr`. Best effort: a drop cannot fail, so if the ring has
    /// no room for the eviction the sounds play out to their end instead,
    /// and the block repays the budget when the last of them lets go.
    fn drop(&mut self) {
        let _ = self.renderer.evict(BufferAddr::of(&self.audio));
    }
}
