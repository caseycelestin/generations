//! Where a project is kept.
//!
//! There is no in-memory copy of a project. A [`Project`] holds its storage
//! and every read and write goes through it, so the project is the disk.
//! Storage answers questions and does what it is told; the rules about
//! what is allowed live in the project. What is behind the trait (a
//! directory today, perhaps a database later) is the implementation's
//! private business.
//!
//! [`Project`]: crate::project::Project

use std::error::Error;
use std::path::Path;

use thiserror::Error as ThisError;

use crate::audio::AudioBuffer;
use crate::project::{Sample, SampleId};

/// Reasons storage cannot do what was asked.
#[derive(Debug, ThisError)]
pub enum StorageError {
    /// Something failed underneath: the filesystem, a codec, a database.
    #[error(transparent)]
    Backend(Box<dyn Error + Send + Sync>),
}

/// Durable storage for one project.
pub trait Storage {
    /// Whether a project is stored here.
    fn exists(&self) -> Result<bool, StorageError>;

    /// Marks this storage as holding a new, empty project.
    fn create(&mut self) -> Result<(), StorageError>;

    /// Everything stored about the sample with this id except its audio,
    /// or `None` if there is none.
    fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError>;

    /// The stored audio of the sample with this id, or `None` if there is
    /// none.
    fn audio(&self, id: SampleId) -> Result<Option<AudioBuffer>, StorageError>;

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
/// This is the one place that decides what a path means. Everything above
/// it sees only the trait.
pub fn open(dir: &Path) -> Result<Box<dyn Storage>, StorageError> {
    let _ = dir;
    todo!("no storage backend yet")
}
