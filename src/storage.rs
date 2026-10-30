//! Where a project is kept between runs.
//!
//! The engine talks to storage through this trait and never learns what is
//! behind it, so the on-disk form (a directory today, perhaps a database
//! later) is the implementation's private business.

use std::error::Error;

use thiserror::Error as ThisError;

use crate::project::Project;

/// Reasons storage cannot do what was asked.
#[derive(Debug, ThisError)]
pub enum StorageError {
    /// Something failed underneath: the filesystem, a codec, a database.
    #[error(transparent)]
    Backend(Box<dyn Error + Send + Sync>),
}

/// Durable storage for one project.
pub trait Storage {
    /// The stored project, or a new empty one if nothing is stored yet.
    fn load_or_create(&mut self) -> Result<Project, StorageError>;
}
