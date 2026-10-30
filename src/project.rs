//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over a [`Storage`]. It keeps no copy of the
//! data: every read and write goes to storage, so changing the project is
//! changing the disk and there is no save step. Today there are no rules
//! and no data; samples, patterns and settings arrive as their steps come.

use crate::storage::{Storage, StorageError};

/// One project, live on its storage.
pub struct Project {
    #[expect(dead_code, reason = "read and written once the project has data")]
    storage: Box<dyn Storage>,
}

impl Project {
    /// The project in `storage`, or a new empty one created there if
    /// nothing is stored yet.
    pub fn get_or_create(mut storage: Box<dyn Storage>) -> Result<Self, StorageError> {
        if !storage.exists()? {
            storage.create()?;
        }
        Ok(Self { storage })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    /// Storage that remembers only whether a project is in it, and refuses
    /// to create one twice.
    struct Memory {
        exists: Rc<Cell<bool>>,
    }

    impl Storage for Memory {
        fn exists(&self) -> Result<bool, StorageError> {
            Ok(self.exists.get())
        }

        fn create(&mut self) -> Result<(), StorageError> {
            assert!(
                !self.exists.get(),
                "create called on storage that already holds a project"
            );
            self.exists.set(true);
            Ok(())
        }
    }

    fn memory(exists: bool) -> (Box<dyn Storage>, Rc<Cell<bool>>) {
        let flag = Rc::new(Cell::new(exists));
        (
            Box::new(Memory {
                exists: flag.clone(),
            }),
            flag,
        )
    }

    #[test]
    fn empty_storage_gets_a_project_created_in_it() {
        let (storage, exists) = memory(false);
        Project::get_or_create(storage).unwrap();
        assert!(exists.get());
    }

    #[test]
    fn existing_project_is_opened_without_creating() {
        let (storage, _exists) = memory(true);
        Project::get_or_create(storage).unwrap();
    }
}
