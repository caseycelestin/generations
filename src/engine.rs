//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! An engine starts from a path. The path becomes a [`Storage`], the
//! storage becomes a [`Project`], and the engine runs on the project.
//!
//! [`Storage`]: crate::storage::Storage

use std::path::Path;

use crate::project::Project;
use crate::storage::{self, StorageError};

/// The engine for one open project.
pub struct Engine {
    project: Project,
}

impl Engine {
    /// An engine on the project at `dir`, created there if none exists.
    pub fn open(dir: &Path) -> Result<Self, StorageError> {
        let storage = storage::open(dir)?;
        Ok(Self {
            project: Project::get_or_create(storage)?,
        })
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }
}
