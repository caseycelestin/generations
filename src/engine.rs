//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! The engine owns a [`Project`] and knows how to print. It is the only code
//! that touches both the project and the effects; the project does not know
//! effects exist and effects do not know projects exist.
//!
//! An engine runs on a [`Storage`]: the place its project is kept.

use crate::audio::AudioBuffer;
use crate::project::{Project, ProjectError};
use crate::sample::SampleId;
use crate::storage::{Storage, StorageError};

/// The engine for one open project.
pub struct Engine {
    project: Project,
    #[expect(dead_code, reason = "written to once changes are stored")]
    storage: Box<dyn Storage>,
}

impl Engine {
    /// An engine on the project in `storage`, which is created if nothing
    /// is stored yet.
    pub fn open(mut storage: Box<dyn Storage>) -> Result<Self, StorageError> {
        Ok(Self {
            project: storage.load_or_create()?,
            storage,
        })
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Adds audio to the project as a new root.
    ///
    /// This is what a file import or a finished recording calls; once the
    /// audio is in, the two are indistinguishable.
    pub fn import(&mut self, audio: AudioBuffer) -> SampleId {
        self.project
            .add(None, audio)
            .expect("a root has no parent to check")
    }

    /// Prints `effect` applied to `source` as the next generation of its
    /// lineage, and returns the new sample's id.
    ///
    /// The source is untouched. On an unknown `source` nothing is added.
    pub fn print(
        &mut self,
        source: SampleId,
        effect: impl FnOnce(&AudioBuffer) -> AudioBuffer,
    ) -> Result<SampleId, ProjectError> {
        let input = self
            .project
            .get(source)
            .ok_or(ProjectError::UnknownSample(source))?;
        let output = effect(input.audio());
        self.project.add(Some(source), output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::reverse;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    /// Storage that holds nothing. Enough to open an engine on.
    struct Empty;

    impl Storage for Empty {
        fn load_or_create(&mut self) -> Result<Project, StorageError> {
            Ok(Project::new())
        }
    }

    fn engine() -> Engine {
        Engine::open(Box::new(Empty)).unwrap()
    }

    #[test]
    fn open_on_empty_storage_gives_an_empty_project() {
        assert_eq!(engine().project().iter().count(), 0);
    }

    #[test]
    fn import_creates_a_root() {
        let mut e = engine();
        let id = e.import(ramp(4));
        let s = e.project().get(id).unwrap();
        assert_eq!(s.parent(), None);
        assert_eq!(s.audio().frames(), 4);
    }

    #[test]
    fn print_adds_a_child_with_the_effect_applied() {
        let mut e = engine();
        let root = e.import(ramp(4));
        let child = e.print(root, reverse).unwrap();

        let c = e.project().get(child).unwrap();
        assert_eq!(c.parent(), Some(root));
        assert_eq!(c.audio().samples(), &[3, 2, 1, 0]);
        // The source is untouched.
        assert_eq!(
            e.project().get(root).unwrap().audio().samples(),
            &[0, 1, 2, 3]
        );
        assert_eq!(e.project().iter().count(), 2);
    }

    #[test]
    fn printing_twice_from_one_source_branches() {
        let mut e = engine();
        let root = e.import(ramp(4));
        let a = e.print(root, reverse).unwrap();
        let b = e.print(root, reverse).unwrap();
        assert_ne!(a, b);
        assert_eq!(e.project().get(a).unwrap().parent(), Some(root));
        assert_eq!(e.project().get(b).unwrap().parent(), Some(root));
    }

    #[test]
    fn print_from_unknown_source_adds_nothing() {
        let mut e = engine();
        let root = e.import(ramp(4));
        let bogus = SampleId(root.raw() + 1);
        assert_eq!(
            e.print(bogus, reverse).unwrap_err(),
            ProjectError::UnknownSample(bogus)
        );
        assert_eq!(e.project().iter().count(), 1);
    }
}
