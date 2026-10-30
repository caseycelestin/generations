//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the code representation of the data on disk, bound to
//! the [`Storage`] it came from. Changing the project changes the disk;
//! there is no save step. Today the data is empty; samples, patterns and
//! settings arrive as their steps come.

use crate::storage::{Storage, StorageError};

/// The data a project is made of. What storage keeps.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProjectData {}

/// One project, live: its data and the storage that data is written to.
pub struct Project {
    data: ProjectData,
    #[expect(
        dead_code,
        reason = "written through once the project has data that changes"
    )]
    storage: Box<dyn Storage>,
}

impl Project {
    /// The project in `storage`, or a new empty one created there if
    /// nothing is stored yet.
    pub fn get_or_create(mut storage: Box<dyn Storage>) -> Result<Self, StorageError> {
        let data = match storage.load()? {
            Some(data) => data,
            None => {
                storage.create()?;
                ProjectData::default()
            }
        };
        Ok(Self { data, storage })
    }

    /// The project's data.
    pub fn data(&self) -> &ProjectData {
        &self.data
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Storage that remembers only whether a project was created in it.
    #[derive(Default)]
    pub(crate) struct Memory {
        pub(crate) stored: Option<ProjectData>,
    }

    impl Storage for Memory {
        fn load(&mut self) -> Result<Option<ProjectData>, StorageError> {
            Ok(self.stored.take())
        }

        fn create(&mut self) -> Result<(), StorageError> {
            self.stored = Some(ProjectData::default());
            Ok(())
        }
    }

    #[test]
    fn empty_storage_gets_a_new_project_created_in_it() {
        let p = Project::get_or_create(Box::new(Memory::default())).unwrap();
        assert_eq!(*p.data(), ProjectData::default());
    }

    #[test]
    fn stored_data_is_the_project() {
        let m = Memory {
            stored: Some(ProjectData {}),
        };
        let p = Project::get_or_create(Box::new(m)).unwrap();
        assert_eq!(*p.data(), ProjectData {});
    }
}
