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
//! `play`, which takes it by reference and shares the audio with the
//! sound it starts; so is `stop`, which cuts every sound playing it.
//! Unloading is dropping it: the sounds still playing let go of the audio
//! as they end, or at once if they were stopped first. A loaded sample
//! made by one project can be heard on any renderer, and the project and
//! the renderer never name each other.
//!
//! The project records nothing about what is loaded. What to load when a
//! project is opened is the caller's to remember, until there is a panel
//! with an opinion.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::dsp::BufferAddress;
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
    pub(crate) fn addr(&self) -> BufferAddress {
        BufferAddress::of(&self.audio)
    }
}
