//! Where a project is kept between runs.
//!
//! A [`Project`] owns its storage and writes through it as it changes, so
//! the project in memory is always the project on disk. Storage never
//! learns what a project means; it keeps and returns [`ProjectData`].
//! What is behind the trait (a directory today, perhaps a database later)
//! is the implementation's private business.
//!
//! [`Project`]: crate::project::Project

use std::error::Error;
use std::path::Path;

use thiserror::Error as ThisError;

use crate::project::ProjectData;

/// Reasons storage cannot do what was asked.
#[derive(Debug, ThisError)]
pub enum StorageError {
    /// Something failed underneath: the filesystem, a codec, a database.
    #[error(transparent)]
    Backend(Box<dyn Error + Send + Sync>),
}

/// Durable storage for one project.
pub trait Storage {
    /// The stored project data, or `None` if nothing is stored here yet.
    fn load(&mut self) -> Result<Option<ProjectData>, StorageError>;

    /// Marks this storage as holding a new, empty project.
    fn create(&mut self) -> Result<(), StorageError>;
}

/// Storage for the project at `dir`.
///
/// This is the one place that decides what a path means. Everything above
/// it sees only the trait.
pub fn open(dir: &Path) -> Result<Box<dyn Storage>, StorageError> {
    let _ = dir;
    todo!("no storage backend yet")
}
