//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! An engine starts from a path. The path becomes a [`Storage`], the
//! storage becomes a [`Project`], and the engine runs on the project. The
//! engine is the only code that touches both the project and the effects;
//! the project does not know effects exist and effects do not know
//! projects exist.
//!
//! [`Storage`]: crate::storage::Storage

use std::path::Path;

use crate::audio::AudioBuffer;
use crate::project::{Project, ProjectError, SampleId};
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

    /// An engine on an already open project.
    #[cfg(test)]
    pub(crate) fn on(project: Project) -> Self {
        Self { project }
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Adds audio to the project as a new root.
    ///
    /// This is what a file import or a finished recording calls; once the
    /// audio is in, the two are indistinguishable.
    pub fn import(&mut self, audio: &AudioBuffer) -> Result<SampleId, ProjectError> {
        self.project.add_sample(None, audio)
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
        let input = self.project.audio(source)?;
        let output = effect(&input);
        self.project.add_sample(Some(source), &output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::reverse;
    use crate::project::tests::memory_project;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    fn engine() -> Engine {
        Engine::on(memory_project())
    }

    #[test]
    fn import_creates_a_root() {
        let mut e = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert_eq!(e.project().sample(id).unwrap().parent, None);
        assert_eq!(e.project().audio(id).unwrap(), ramp(4));
    }

    #[test]
    fn print_adds_a_child_with_the_effect_applied() {
        let mut e = engine();
        let root = e.import(&ramp(4)).unwrap();
        let child = e.print(root, reverse).unwrap();

        assert_eq!(e.project().sample(child).unwrap().parent, Some(root));
        assert_eq!(e.project().audio(child).unwrap().samples(), &[3, 2, 1, 0]);
        // The source is untouched.
        assert_eq!(e.project().audio(root).unwrap().samples(), &[0, 1, 2, 3]);
    }

    #[test]
    fn printing_twice_from_one_source_branches() {
        let mut e = engine();
        let root = e.import(&ramp(4)).unwrap();
        let a = e.print(root, reverse).unwrap();
        let b = e.print(root, reverse).unwrap();
        assert_ne!(a, b);
        assert_eq!(e.project().sample(a).unwrap().parent, Some(root));
        assert_eq!(e.project().sample(b).unwrap().parent, Some(root));
    }

    #[test]
    fn print_from_unknown_source_adds_nothing() {
        let mut e = engine();
        let root = e.import(&ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.print(bogus, reverse),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert!(e.project().sample(bogus).is_err());
    }
}
