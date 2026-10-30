//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over a [`Storage`]. It keeps no copy of the
//! data: every read and write goes to storage, so changing the project is
//! changing the disk and there is no save step. Today a project holds
//! samples; patterns and settings arrive as their steps come.

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::storage::{Storage, StorageError};

/// Identity of a sample within one project.
///
/// A sample does not exist until it is stored, so storage mints the id
/// when it adds one, from a per-project counter. Ids are never reused and
/// are not meaningful across projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Where one sample sits in the tree.
///
/// "Generation" is not a type. It is the relationship between a sample and
/// its parent: the sample one generation behind it. Lineage is the tree
/// formed by those pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub id: SampleId,
    /// The sample one generation behind this one. `None` for a root.
    pub parent: Option<SampleId>,
}

/// Reasons a project operation can fail.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// No sample with this id exists in the project.
    #[error("no sample with id {}", .0.0)]
    UnknownSample(SampleId),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// One project, live on its storage.
pub struct Project {
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

    /// Adds audio as a sample and returns its id.
    ///
    /// With no parent it starts a new lineage; with one it is the next
    /// generation of that lineage. An unknown parent is refused and
    /// nothing is stored.
    pub fn add_sample(
        &mut self,
        parent: Option<SampleId>,
        audio: &AudioBuffer,
    ) -> Result<SampleId, ProjectError> {
        if let Some(p) = parent {
            self.sample(p)?;
        }
        Ok(self.storage.add_sample(parent, audio)?)
    }

    /// Where the sample with this id sits in the tree.
    pub fn sample(&self, id: SampleId) -> Result<Sample, ProjectError> {
        self.storage
            .sample(id)?
            .ok_or(ProjectError::UnknownSample(id))
    }

    /// The audio of the sample with this id, read from storage.
    pub fn audio(&self, id: SampleId) -> Result<AudioBuffer, ProjectError> {
        self.storage
            .audio(id)?
            .ok_or(ProjectError::UnknownSample(id))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// What the stub storage remembers.
    #[derive(Default)]
    struct State {
        exists: bool,
        next_id: u32,
        samples: Vec<(Sample, AudioBuffer)>,
    }

    /// In-memory storage for tests, shared with the test through `Rc` so
    /// it can be inspected after the project takes ownership.
    struct Memory(Rc<RefCell<State>>);

    impl Storage for Memory {
        fn exists(&self) -> Result<bool, StorageError> {
            Ok(self.0.borrow().exists)
        }

        fn create(&mut self) -> Result<(), StorageError> {
            let mut s = self.0.borrow_mut();
            assert!(
                !s.exists,
                "create called on storage that already holds a project"
            );
            s.exists = true;
            Ok(())
        }

        fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError> {
            Ok(self.find(id).map(|(s, _)| s))
        }

        fn audio(&self, id: SampleId) -> Result<Option<AudioBuffer>, StorageError> {
            Ok(self.find(id).map(|(_, a)| a))
        }

        fn add_sample(
            &mut self,
            parent: Option<SampleId>,
            audio: &AudioBuffer,
        ) -> Result<SampleId, StorageError> {
            let mut s = self.0.borrow_mut();
            let id = SampleId(s.next_id);
            s.next_id += 1;
            s.samples.push((Sample { id, parent }, audio.clone()));
            Ok(id)
        }
    }

    impl Memory {
        fn find(&self, id: SampleId) -> Option<(Sample, AudioBuffer)> {
            self.0
                .borrow()
                .samples
                .iter()
                .find(|(s, _)| s.id == id)
                .cloned()
        }
    }

    fn project(exists: bool) -> (Project, Rc<RefCell<State>>) {
        let state = Rc::new(RefCell::new(State {
            exists,
            ..State::default()
        }));
        let p = Project::get_or_create(Box::new(Memory(state.clone()))).unwrap();
        (p, state)
    }

    fn tone() -> AudioBuffer {
        AudioBuffer::new(1, vec![0, 1, 2, 3]).unwrap()
    }

    fn structure(state: &Rc<RefCell<State>>) -> Vec<(SampleId, Option<SampleId>)> {
        state
            .borrow()
            .samples
            .iter()
            .map(|(s, _)| (s.id, s.parent))
            .collect()
    }

    #[test]
    fn empty_storage_gets_a_project_created_in_it() {
        let (_p, state) = project(false);
        assert!(state.borrow().exists);
    }

    #[test]
    fn existing_project_is_opened_without_creating() {
        let (_p, state) = project(true);
        assert!(state.borrow().exists);
    }

    #[test]
    fn add_without_parent_stores_a_root() {
        let (mut p, state) = project(false);
        let id = p.add_sample(None, &tone()).unwrap();
        assert_eq!(structure(&state), vec![(id, None)]);
    }

    #[test]
    fn add_with_parent_stores_the_next_generation() {
        let (mut p, state) = project(false);
        let root = p.add_sample(None, &tone()).unwrap();
        let child = p.add_sample(Some(root), &tone()).unwrap();
        assert_ne!(root, child);
        assert_eq!(structure(&state), vec![(root, None), (child, Some(root))]);
    }

    #[test]
    fn a_stored_sample_reads_back() {
        let (mut p, _state) = project(false);
        let root = p.add_sample(None, &tone()).unwrap();
        let child = p.add_sample(Some(root), &tone()).unwrap();
        assert_eq!(
            p.sample(child).unwrap(),
            Sample {
                id: child,
                parent: Some(root)
            }
        );
        assert_eq!(p.audio(child).unwrap(), tone());
    }

    #[test]
    fn reading_an_unknown_sample_is_refused() {
        let (p, _state) = project(false);
        let bogus = SampleId(7);
        assert!(matches!(p.sample(bogus), Err(ProjectError::UnknownSample(id)) if id == bogus));
        assert!(matches!(p.audio(bogus), Err(ProjectError::UnknownSample(id)) if id == bogus));
    }

    #[test]
    fn add_with_unknown_parent_is_refused_and_stores_nothing() {
        let (mut p, state) = project(false);
        let bogus = SampleId(99);
        assert!(matches!(
            p.add_sample(Some(bogus), &tone()),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert!(structure(&state).is_empty());
    }
}
