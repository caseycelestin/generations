//! A loaded sample: its audio in RAM, owned by whoever holds it.
//!
//! [`Project::load`](crate::project::Project::load) reads a sample's audio
//! into a block on the ledger and returns it as a [`LoadedSample`]. That
//! value is the audio's owner. Hold it and the sample is in RAM; drop it
//! and the block repays the budget once the last sound playing it has let
//! go. There is no list of loaded samples anywhere else: what is loaded is
//! what is held, which is how Rust says it, a `File` is open while you
//! hold it.
//!
//! It is data, not a handle on anything: it knows nothing about the
//! renderer or whether it is playing. Hearing it is the renderer's verb,
//! which takes it by reference and shares the audio with the sound it
//! starts; cutting those sounds and letting the audio go at once is the
//! renderer's too, taking it by value. So a loaded sample made by one
//! project can be heard on any renderer, and the project and the renderer
//! never name each other.
//!
//! The project records nothing about what is loaded. What to load when a
//! project is opened is the caller's to remember, until there is a panel
//! with an opinion.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::dsp::BufferAddr;
use crate::project::SampleId;

/// A sample's audio in RAM. Owning this is what loaded means.
#[derive(Debug)]
pub struct LoadedSample {
    id: SampleId,
    /// Immutable behind an `Arc` so a sound can hold it while it plays;
    /// the render side only ever sees the clones it was handed.
    audio: Arc<AudioBuffer>,
}

impl LoadedSample {
    /// The sample `id`, whose audio is `audio`.
    pub(crate) fn new(id: SampleId, audio: AudioBuffer) -> Self {
        Self {
            id,
            audio: Arc::new(audio),
        }
    }

    /// Which sample this is.
    pub fn id(&self) -> SampleId {
        self.id
    }

    /// The audio, shared: what a sound playing it holds.
    pub(crate) fn audio(&self) -> &Arc<AudioBuffer> {
        &self.audio
    }

    /// Where the audio lives, the key an eviction names it by.
    pub(crate) fn addr(&self) -> BufferAddr {
        BufferAddr::of(&self.audio)
    }
}
