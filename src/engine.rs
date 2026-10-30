//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! The engine owns the [`Project`] and the [`Store`] it lives in. It is the
//! only code that touches both, and the only code that touches effects: the
//! project does not know effects or stores exist, and effects do not know
//! projects exist.
//!
//! Every change is in the store before the call returns. There is no save
//! step for the active project: on the device the store is on eMMC and a
//! power-down loses nothing that was done.

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::project::{Project, ProjectError};
use crate::sample::SampleId;
use crate::store::{Store, StoreError};

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The engine for one active project.
pub struct Engine {
    project: Project,
    store: Box<dyn Store>,
}

impl Engine {
    /// An engine on the project in `store`.
    pub fn open(store: Box<dyn Store>) -> Result<Self, StoreError> {
        Ok(Self {
            project: store.load()?,
            store,
        })
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// A sample's audio, read from the store.
    pub fn audio(&self, id: SampleId) -> Result<AudioBuffer, EngineError> {
        self.project
            .get(id)
            .ok_or(ProjectError::UnknownSample(id))?;
        Ok(self.store.get(id)?)
    }

    /// Adds audio to the project as a new root.
    ///
    /// This is what a file import or a finished recording calls; once the
    /// audio is in, the two are indistinguishable.
    pub fn import(&mut self, audio: AudioBuffer) -> Result<SampleId, EngineError> {
        self.add(None, &audio)
    }

    /// Prints `effect` applied to `source` as the next generation of its
    /// lineage, and returns the new sample's id.
    ///
    /// The source is untouched. On an unknown `source` nothing is added.
    pub fn print(
        &mut self,
        source: SampleId,
        effect: impl FnOnce(&AudioBuffer) -> AudioBuffer,
    ) -> Result<SampleId, EngineError> {
        let input = self.audio(source)?;
        let output = effect(&input);
        self.add(Some(source), &output)
    }

    /// Records a new sample in the project and the store together.
    ///
    /// If the store refuses, the sample is taken back out of the project so
    /// memory and store agree.
    fn add(
        &mut self,
        parent: Option<SampleId>,
        audio: &AudioBuffer,
    ) -> Result<SampleId, EngineError> {
        let id = self.project.add(parent)?;
        if let Err(e) = self.store.put(id, audio, &self.project) {
            self.project
                .remove(id)
                .expect("just added, has no children");
            return Err(e.into());
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::FileStore;
    use crate::effects::reverse;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    fn open(dir: &std::path::Path) -> Engine {
        Engine::open(Box::new(FileStore::open(dir).unwrap())).unwrap()
    }

    #[test]
    fn import_creates_a_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        let id = e.import(ramp(4)).unwrap();
        assert_eq!(e.project().get(id).unwrap().parent(), None);
        assert_eq!(e.audio(id).unwrap().frames(), 4);
    }

    #[test]
    fn print_adds_a_child_with_the_effect_applied() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        let root = e.import(ramp(4)).unwrap();
        let child = e.print(root, reverse).unwrap();

        assert_eq!(e.project().get(child).unwrap().parent(), Some(root));
        assert_eq!(e.audio(child).unwrap().samples(), &[3, 2, 1, 0]);
        // The source is untouched.
        assert_eq!(e.audio(root).unwrap().samples(), &[0, 1, 2, 3]);
        assert_eq!(e.project().iter().count(), 2);
    }

    #[test]
    fn printing_twice_from_one_source_branches() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        let root = e.import(ramp(4)).unwrap();
        let a = e.print(root, reverse).unwrap();
        let b = e.print(root, reverse).unwrap();
        assert_ne!(a, b);
        assert_eq!(e.project().get(a).unwrap().parent(), Some(root));
        assert_eq!(e.project().get(b).unwrap().parent(), Some(root));
    }

    #[test]
    fn every_change_is_stored_without_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        let root = e.import(ramp(4)).unwrap();
        let child = e.print(root, reverse).unwrap();
        drop(e);

        let e = open(dir.path());
        assert_eq!(e.project().iter().count(), 2);
        assert_eq!(e.project().get(child).unwrap().parent(), Some(root));
        assert_eq!(e.audio(child).unwrap().samples(), &[3, 2, 1, 0]);
    }

    #[test]
    fn print_from_unknown_source_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = open(dir.path());
        let root = e.import(ramp(4)).unwrap();
        let bogus = SampleId(root.raw() + 1);
        assert!(matches!(
            e.print(bogus, reverse),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert_eq!(e.project().iter().count(), 1);
    }
}
