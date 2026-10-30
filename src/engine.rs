//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! The engine owns a [`Project`] and knows how to print. It is the only code
//! that touches both the project and the effects; the project does not know
//! effects exist and effects do not know projects exist.
//!
//! The engine also owns a working directory and writes through to it after
//! every change, so the project survives a power-down without anyone saving.
//! On the device that directory is on eMMC. [`Engine::save`] copies the
//! project somewhere else, the SD card, and is the only save a user sees.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::disk::{self, DiskError};
use crate::project::{Project, ProjectError};
use crate::sample::SampleId;

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Disk(#[from] DiskError),
}

/// The engine for one open project.
#[derive(Debug)]
pub struct Engine {
    project: Project,
    dir: PathBuf,
}

impl Engine {
    /// An engine on a new, empty project with `dir` as its working
    /// directory. The directory is created and written at once.
    pub fn create(dir: &Path) -> Result<Self, DiskError> {
        let engine = Self {
            project: Project::new(),
            dir: dir.to_path_buf(),
        };
        disk::save(&engine.project, &engine.dir)?;
        Ok(engine)
    }

    /// An engine on the project in `dir`, which becomes its working
    /// directory.
    pub fn open(dir: &Path) -> Result<Self, DiskError> {
        Ok(Self {
            project: disk::load(dir)?,
            dir: dir.to_path_buf(),
        })
    }

    /// Copies the project to `dir`, creating it if needed.
    ///
    /// This is the user's save: the working directory is already current.
    pub fn save(&self, dir: &Path) -> Result<(), DiskError> {
        disk::save(&self.project, dir)
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The directory the engine writes through to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Adds audio to the project as a new root.
    ///
    /// This is what a file import or a finished recording calls; once the
    /// audio is in, the two are indistinguishable.
    pub fn import(&mut self, audio: AudioBuffer) -> Result<SampleId, EngineError> {
        let id = self
            .project
            .add(None, audio)
            .expect("a root has no parent to check");
        self.write_through()?;
        Ok(id)
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
        let input = self
            .project
            .get(source)
            .ok_or(ProjectError::UnknownSample(source))?;
        let output = effect(input.audio());
        let id = self.project.add(Some(source), output)?;
        self.write_through()?;
        Ok(id)
    }

    /// Brings the working directory up to date with the project.
    ///
    /// If this fails the project in memory is ahead of the one on disk. The
    /// next write-through that succeeds catches up, since a save writes
    /// whatever is missing.
    fn write_through(&self) -> Result<(), DiskError> {
        disk::save(&self.project, &self.dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::reverse;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    fn engine() -> (Engine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::create(dir.path()).unwrap();
        (e, dir)
    }

    #[test]
    fn create_writes_an_empty_project_at_once() {
        let (e, dir) = engine();
        assert_eq!(e.dir(), dir.path());
        assert_eq!(
            Engine::open(dir.path()).unwrap().project().iter().count(),
            0
        );
    }

    #[test]
    fn import_creates_a_root() {
        let (mut e, _dir) = engine();
        let id = e.import(ramp(4)).unwrap();
        let s = e.project().get(id).unwrap();
        assert_eq!(s.parent(), None);
        assert_eq!(s.audio().frames(), 4);
    }

    #[test]
    fn print_adds_a_child_with_the_effect_applied() {
        let (mut e, _dir) = engine();
        let root = e.import(ramp(4)).unwrap();
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
        let (mut e, _dir) = engine();
        let root = e.import(ramp(4)).unwrap();
        let a = e.print(root, reverse).unwrap();
        let b = e.print(root, reverse).unwrap();
        assert_ne!(a, b);
        assert_eq!(e.project().get(a).unwrap().parent(), Some(root));
        assert_eq!(e.project().get(b).unwrap().parent(), Some(root));
    }

    #[test]
    fn every_change_is_on_disk_without_a_save() {
        let (mut e, dir) = engine();
        let root = e.import(ramp(4)).unwrap();
        assert_eq!(
            Engine::open(dir.path()).unwrap().project().iter().count(),
            1
        );

        let child = e.print(root, reverse).unwrap();
        let reopened = Engine::open(dir.path()).unwrap();
        assert_eq!(reopened.project().iter().count(), 2);
        assert_eq!(reopened.project().get(child).unwrap().parent(), Some(root));
        assert_eq!(
            reopened.project().get(child).unwrap().audio().samples(),
            &[3, 2, 1, 0]
        );
    }

    #[test]
    fn save_copies_the_project_elsewhere() {
        let (mut e, _work) = engine();
        let root = e.import(ramp(4)).unwrap();
        let child = e.print(root, reverse).unwrap();

        let card = tempfile::tempdir().unwrap();
        e.save(card.path()).unwrap();
        let copy = Engine::open(card.path()).unwrap();
        assert_eq!(copy.project().iter().count(), 2);
        assert_eq!(copy.project().get(child).unwrap().parent(), Some(root));
    }

    #[test]
    fn print_from_unknown_source_adds_nothing() {
        let (mut e, _dir) = engine();
        let root = e.import(ramp(4)).unwrap();
        let bogus = SampleId(root.raw() + 1);
        assert!(matches!(
            e.print(bogus, reverse),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert_eq!(e.project().iter().count(), 1);
    }
}
