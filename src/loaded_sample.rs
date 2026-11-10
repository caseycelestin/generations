//! A loaded sample: its audio in RAM, owned by whoever holds the handle.
//!
//! `Engine::load` reads a sample's audio into a block on the ledger and
//! returns it as a [`LoadedSample`]. The handle is the audio's owner.
//! Hold it and the sample is in RAM; drop it and the sample is unloaded,
//! the block repaying the budget once the last sound playing it has let
//! go. There is no list of loaded samples anywhere else: what is loaded
//! is what is held, which is how Rust says it, a `File` is open while
//! you hold it.
//!
//! Playing is a verb on the loaded sample, because it needs nothing but
//! the audio, a fresh sound id and the command ring, and the handle
//! carries all three. `Engine::unload` takes the handle back to do the
//! two things a drop cannot: cut the sounds playing it, and clear the
//! sample's loaded flag in the project.
//!
//! The flag the project keeps per sample, `loaded`, is what the caller
//! asked for last: set by `load`, cleared by `unload`, left alone by a
//! drop. `Project::loaded_samples` is therefore what to load on open,
//! not what is in RAM; the engine loads nothing on its own.

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

    /// Where the audio lives, the key an eviction names it by.
    pub(crate) fn addr(&self) -> BufferAddr {
        BufferAddr::of(&self.audio)
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
