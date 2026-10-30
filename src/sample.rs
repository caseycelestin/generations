//! A printed piece of audio and where it came from.
//!
//! "Generation" is not a type. It is the relationship between a [`Sample`]
//! and its `parent`: the sample one generation behind it. Lineage is the
//! tree formed by those pointers inside a [`crate::project::Project`].
//!
//! A sample has no name and no notion of being "current". Those belong to
//! whatever points at a sample (a pad, a track) and are built in a later
//! layer. Samples themselves never change once created.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

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

/// How a sample came to exist.
///
/// This is a human-readable record, not a set of parameters. You can read
/// the history of a sample but you cannot reopen a step and tweak it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Copied into the project from an external file. The path is kept as a
    /// label for the user and is never resolved again.
    Import { path: PathBuf },
    /// Captured from an input device.
    Record { device: String },
    /// Printed from its parent by applying some operation.
    Print { op: String },
}

/// One immutable piece of audio in a project.
#[derive(Debug, Clone)]
pub struct Sample {
    id: SampleId,
    parent: Option<SampleId>,
    origin: Origin,
    created_at: SystemTime,
    audio: Arc<AudioBuffer>,
}

impl Sample {
    pub(crate) fn new(
        id: SampleId,
        parent: Option<SampleId>,
        origin: Origin,
        audio: Arc<AudioBuffer>,
    ) -> Self {
        Self { id, parent, origin, created_at: SystemTime::now(), audio }
    }

    pub fn id(&self) -> SampleId {
        self.id
    }

    /// The sample one generation behind this one, if any.
    pub fn parent(&self) -> Option<SampleId> {
        self.parent
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }

    /// The audio. Returned by reference so that a later change to how audio
    /// is stored (lazy loading, views into a parent) stays inside this type.
    pub fn audio(&self) -> &AudioBuffer {
        &self.audio
    }

    /// True for a root of a lineage: an import or a recording.
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }
}
