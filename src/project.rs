//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over a `Storage`. It keeps no copy of the
//! data: every read and write goes to storage, so changing the project is
//! changing the disk and there is no save step. Today a project holds
//! samples; patterns and settings arrive as their steps come.

use thiserror::Error;

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::memory::Ledger;
use crate::storage::{Storage, StorageError};

/// Identity of a sample within one project.
///
/// A sample does not exist until it is stored, so storage mints the id
/// when it adds one, from a per-project counter. Ids are never reused and
/// are not meaningful across projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Everything stored about one sample except its audio, which never
/// leaves the engine: it is heard through playback and changed through a
/// print. More fields arrive as the project grows.
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
///
/// A caller reaches it read-only through `Engine::project()`. Everything
/// that changes it is an engine method, so the engine can keep the other
/// roles in step (an import passes through the ledger on its way here).
pub struct Project {
    storage: Box<dyn Storage>,
}

impl Project {
    /// The project in `storage`. A storage that exists holds a whole,
    /// valid project, so there is nothing to check here.
    pub(crate) fn new(storage: Box<dyn Storage>) -> Self {
        Self { storage }
    }

    /// Adds audio as a sample and returns its id.
    ///
    /// With no parent it starts a new lineage; with one it is the next
    /// generation of that lineage. An unknown parent is refused and
    /// nothing is stored.
    pub(crate) fn add_sample(
        &mut self,
        parent: Option<SampleId>,
        audio: &AudioBuffer,
    ) -> Result<SampleId, ProjectError> {
        if let Some(p) = parent {
            self.check(p)?;
        }
        Ok(self.storage.add_sample(parent, audio)?)
    }

    /// Refuses an id no sample has. Asks only whether the record exists,
    /// not for the record, so it still works for a sample whose audio has
    /// gone bad on disk: that sample must stay a valid parent to point at
    /// and, later, removable.
    fn check(&self, id: SampleId) -> Result<(), ProjectError> {
        if self.storage.has_sample(id)? {
            Ok(())
        } else {
            Err(ProjectError::UnknownSample(id))
        }
    }

    /// Everything stored about the sample with this id, except its audio.
    pub fn sample(&self, id: SampleId) -> Result<Sample, ProjectError> {
        self.storage
            .sample(id)?
            .ok_or(ProjectError::UnknownSample(id))
    }

    /// The audio of the sample with this id, read from storage.
    ///
    /// Crate-private because reading allocates the whole buffer against
    /// `ledger`; the engine holds that and is the one place a caller gets
    /// audio from.
    pub(crate) fn audio(
        &self,
        id: SampleId,
        ledger: &Arc<Ledger>,
    ) -> Result<AudioBuffer, ProjectError> {
        self.storage
            .audio(id, ledger)?
            .ok_or(ProjectError::UnknownSample(id))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::config::Pcm;
    use crate::testing::{audio, unlimited};

    /// What the stub storage remembers.
    #[derive(Default)]
    pub(crate) struct State {
        next_id: u32,
        /// Each sample with its channel count and audio as plain samples:
        /// the backend's own copy, outside the budget, as a disk is.
        samples: Vec<(Sample, u16, Vec<Pcm>)>,
    }

    /// In-memory storage for tests, shared with the test through `Rc` so
    /// it can be inspected after the project takes ownership.
    struct Memory(Rc<RefCell<State>>);

    impl Storage for Memory {
        fn has_sample(&self, id: SampleId) -> Result<bool, StorageError> {
            Ok(self.find(id).is_some())
        }

        fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError> {
            Ok(self.find(id).map(|(s, _, _)| s))
        }

        fn audio(
            &self,
            id: SampleId,
            ledger: &Arc<Ledger>,
        ) -> Result<Option<AudioBuffer>, StorageError> {
            let Some((_, channels, samples)) = self.find(id) else {
                return Ok(None);
            };
            let mut block = ledger
                .alloc(samples.len())
                .map_err(|source| StorageError::OverBudget { id, source })?;
            block.extend(samples);
            Ok(Some(AudioBuffer::new(channels, block).unwrap()))
        }

        fn add_sample(
            &mut self,
            parent: Option<SampleId>,
            audio: &AudioBuffer,
        ) -> Result<SampleId, StorageError> {
            let mut s = self.0.borrow_mut();
            let id = SampleId(s.next_id);
            s.next_id += 1;
            s.samples.push((
                Sample { id, parent },
                audio.channels(),
                audio.samples().to_vec(),
            ));
            Ok(id)
        }
    }

    impl Memory {
        fn find(&self, id: SampleId) -> Option<(Sample, u16, Vec<Pcm>)> {
            self.0
                .borrow()
                .samples
                .iter()
                .find(|(s, _, _)| s.id == id)
                .cloned()
        }
    }

    /// A project on in-memory storage, for tests elsewhere in the crate.
    pub(crate) fn memory_project() -> Project {
        project().0
    }

    /// A project on in-memory storage, and the storage's state so the test
    /// can look at what the project did to it.
    pub(crate) fn project() -> (Project, Rc<RefCell<State>>) {
        let state = Rc::new(RefCell::new(State::default()));
        let p = Project::new(Box::new(Memory(state.clone())));
        (p, state)
    }

    fn tone() -> AudioBuffer {
        audio(1, &[0.0, 0.1, 0.2, 0.3])
    }

    fn structure(state: &Rc<RefCell<State>>) -> Vec<(SampleId, Option<SampleId>)> {
        state
            .borrow()
            .samples
            .iter()
            .map(|(s, _, _)| (s.id, s.parent))
            .collect()
    }

    #[test]
    fn add_without_parent_stores_a_root() {
        let (mut p, state) = project();
        let id = p.add_sample(None, &tone()).unwrap();
        assert_eq!(structure(&state), vec![(id, None)]);
    }

    #[test]
    fn add_with_parent_stores_the_next_generation() {
        let (mut p, state) = project();
        let root = p.add_sample(None, &tone()).unwrap();
        let child = p.add_sample(Some(root), &tone()).unwrap();
        assert_ne!(root, child);
        assert_eq!(structure(&state), vec![(root, None), (child, Some(root))]);
    }

    #[test]
    fn a_stored_sample_reads_back() {
        let (mut p, _state) = project();
        let root = p.add_sample(None, &tone()).unwrap();
        let child = p.add_sample(Some(root), &tone()).unwrap();
        assert_eq!(
            p.sample(child).unwrap(),
            Sample {
                id: child,
                parent: Some(root),
            }
        );
        assert_eq!(
            p.audio(child, &unlimited()).unwrap().samples(),
            tone().samples()
        );
    }

    #[test]
    fn reading_an_unknown_sample_is_refused() {
        let (p, _state) = project();
        let bogus = SampleId(7);
        assert!(matches!(p.sample(bogus), Err(ProjectError::UnknownSample(id)) if id == bogus));
        assert!(matches!(
            p.audio(bogus, &unlimited()),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
    }

    #[test]
    fn add_with_unknown_parent_is_refused_and_stores_nothing() {
        let (mut p, state) = project();
        let bogus = SampleId(99);
        assert!(matches!(
            p.add_sample(Some(bogus), &tone()),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert!(structure(&state).is_empty());
    }
}
