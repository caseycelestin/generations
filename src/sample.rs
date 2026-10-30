//! A printed piece of audio and its place in a lineage.
//!
//! "Generation" is not a type. It is the relationship between a [`Sample`]
//! and its `parent`: the sample one generation behind it. Lineage is the
//! tree formed by those pointers inside a [`crate::project::Project`].
//!
//! A sample has no name, no label, and no notion of being "current". It does
//! not record where it came from either: once audio is in the project, a file
//! import and a recording are the same thing, a root. Naming and organising
//! belong to whatever points at a sample (a pad, a track) and are built in a
//! later layer. Samples themselves never change once created.

use std::sync::Arc;

use crate::audio::AudioBuffer;

/// Identity of a sample within one project.
///
/// Minted from a per-project counter. Not meaningful across projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub(crate) u32);

impl SampleId {
    /// The raw counter value, for persistence and display.
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// One immutable piece of audio in a project.
#[derive(Debug, Clone)]
pub struct Sample {
    id: SampleId,
    parent: Option<SampleId>,
    audio: Arc<AudioBuffer>,
}

impl Sample {
    pub(crate) fn new(id: SampleId, parent: Option<SampleId>, audio: Arc<AudioBuffer>) -> Self {
        Self { id, parent, audio }
    }

    pub fn id(&self) -> SampleId {
        self.id
    }

    /// The sample one generation behind this one. `None` for a root.
    pub fn parent(&self) -> Option<SampleId> {
        self.parent
    }

    /// The audio. Returned by reference so that a later change to how audio
    /// is stored (lazy loading, views into a parent) stays inside this type.
    pub fn audio(&self) -> &AudioBuffer {
        &self.audio
    }
}
