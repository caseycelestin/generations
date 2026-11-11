//! Where a project is kept.
//!
//! There is no in-memory copy of a project. A [`Project`] holds its storage
//! and every read and write goes through it, so the project is the disk.
//! Storage answers questions and does what it is told; the rules about
//! what is allowed live in the project. What is behind the trait (a
//! directory today, perhaps a database later) is the implementation's
//! private business.
//!
//! [`files::FileStorage`] is the one backend so far. A read is a
//! [`Block`] from the ledger, and the WAV on disk holds the engine's
//! sample type.
//!
//! [`Project`]: crate::project::Project
//! [`Block`]: crate::memory::Block

pub mod files;

use std::error::Error;
use std::path::Path;

use thiserror::Error as ThisError;

use crate::audio::AudioBuffer;
use crate::memory::{Ledger, OverBudget};
use crate::project::{Sample, SampleId};

/// Reasons storage cannot do what was asked.
#[derive(Debug, ThisError)]
pub enum StorageError {
    /// The stored project was written by a format this build does not read.
    #[error("stored format version {0} is not supported")]
    UnsupportedVersion(u32),
    /// Something is stored here, and it is not a project at all.
    #[error("storage holds something that is not a project: {0}")]
    NotAProject(String),
    /// Something is stored here, but it is not a whole project.
    #[error("stored project is corrupt: {0}")]
    Corrupt(String),
    /// Stored audio is not in the engine's one fixed format.
    #[error("audio for sample {} is not in the engine's format: {detail}", .id.0)]
    BadAudioFormat { id: SampleId, detail: String },
    /// The sample's audio would put audio memory over its budget.
    /// Nothing was allocated.
    #[error("reading sample {}: {source}", .id.0)]
    OverBudget {
        id: SampleId,
        #[source]
        source: OverBudget,
    },
    /// Something failed underneath: the filesystem, a codec, a database.
    #[error(transparent)]
    Backend(Box<dyn Error + Send + Sync>),
}

/// Durable storage for one project.
///
/// A storage always holds a whole project: opening one creates an empty
/// project where there was nothing, validates what it finds otherwise, and
/// refuses anything broken. So these operations never have to ask whether
/// a project is there.
pub trait Storage {
    /// Whether a sample with this id is stored. Answered from the record
    /// alone, so it holds even when the sample's audio cannot be read.
    fn has_sample(&self, id: SampleId) -> Result<bool, StorageError>;

    /// Everything stored about the sample with this id except its audio,
    /// or `None` if there is none.
    ///
    /// `channels` and `frames` describe the audio [`audio`] will return
    /// for the same id, so a caller can tell what a read would cost
    /// before making it. A backend may need to look at the audio to
    /// answer, so this can fail where the audio is unreadable.
    ///
    /// [`audio`]: Storage::audio
    fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError>;

    /// The stored audio of the sample with this id in a block from
    /// `ledger`, or `None` if there is none.
    ///
    /// The block is sized from what is stored before any audio is read,
    /// so a sample that does not fit is refused with nothing allocated.
    /// The caller holds the audio for as long as it wants the memory; the
    /// bytes go back when the buffer drops.
    fn audio(&self, id: SampleId, ledger: &Ledger) -> Result<Option<AudioBuffer>, StorageError>;

    /// Stores a sample with its audio and returns its new id.
    ///
    /// Ids come from a counter that only goes up, so an id is never
    /// reused, even after the sample it named is removed. The parent is
    /// recorded as given; whether it exists is the project's rule.
    fn add_sample(
        &mut self,
        parent: Option<SampleId>,
        audio: &AudioBuffer,
    ) -> Result<SampleId, StorageError>;
}

/// Storage for the project at `dir`.
///
/// Creates an empty project there if the directory is empty or missing,
/// opens on the project it finds otherwise, and refuses anything else:
/// a broken project, or files that are not a project at all. This is
/// the one place that decides what a path means; everything above it sees
/// only the trait.
pub fn open(dir: &Path) -> Result<Box<dyn Storage>, StorageError> {
    Ok(Box::new(files::FileStorage::open(dir)?))
}
