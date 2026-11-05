//! Thin orchestration over a project: what a UI or front panel calls.
//!
//! An engine starts from a path. The path becomes a [`Storage`], the
//! storage becomes a [`Project`], and the engine runs on the project. The
//! engine is the only code that touches more than one role: the project,
//! the effects, and playback. The project does not know effects or
//! playback exist, and they do not know projects exist.
//!
//! [`Storage`]: crate::storage::Storage

use std::path::Path;

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::SAMPLE_RAM_BUDGET;
use crate::playback::{PlaybackError, ResidentSet};
use crate::project::{Project, ProjectError, SampleId};
use crate::storage::{self, StorageError};

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Playback(#[from] PlaybackError),
}

/// The engine for one open project.
pub struct Engine {
    project: Project,
    resident: ResidentSet,
}

impl Engine {
    /// An engine on the project at `dir`, created there if none exists.
    ///
    /// Every sample the project marks loaded is brought into RAM before
    /// this returns, so the device comes back ready. If they no longer fit
    /// the budget (it shrank, or the project moved to a smaller device)
    /// the open fails rather than deciding which ones to leave out.
    pub fn open(dir: &Path) -> Result<Self, EngineError> {
        Self::open_with_budget(dir, SAMPLE_RAM_BUDGET)
    }

    /// [`Engine::open`] with a budget other than the configured one.
    pub(crate) fn open_with_budget(dir: &Path, budget: usize) -> Result<Self, EngineError> {
        let storage = storage::open(dir)?;
        Self::on(Project::new(storage), ResidentSet::new(budget))
    }

    /// An engine on an already open project, loading what it marks loaded.
    pub(crate) fn on(project: Project, resident: ResidentSet) -> Result<Self, EngineError> {
        let mut engine = Self { project, resident };
        for id in engine.project.loaded_samples()? {
            engine.make_resident(id)?;
        }
        Ok(engine)
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The audio this engine holds in RAM for playback.
    pub fn resident(&self) -> &ResidentSet {
        &self.resident
    }

    /// Adds audio to the project as a new root.
    ///
    /// This is what a file import or a finished recording calls; once the
    /// audio is in, the two are indistinguishable. The new sample is not
    /// loaded; that is a separate decision, so a print or import lands on
    /// disk whether or not it fits in RAM.
    pub fn import(&mut self, audio: &AudioBuffer) -> Result<SampleId, ProjectError> {
        self.project.add_sample(None, audio)
    }

    /// Loads `id`: brings its audio into RAM and marks it loaded, so it
    /// stays loaded across a reopen.
    ///
    /// The audio goes into RAM first and the flag is set only if that
    /// succeeded, so the flag is never true for a sample that is not
    /// resident. A sample that does not fit the budget is refused and
    /// nothing changes. Loading a loaded sample changes nothing.
    pub fn load(&mut self, id: SampleId) -> Result<(), EngineError> {
        self.make_resident(id)?;
        self.project.set_loaded(id, true)?;
        Ok(())
    }

    /// Unloads `id`: clears the flag and drops its audio from RAM. Any
    /// voice still playing it keeps the audio alive until it finishes.
    /// Unloading an unloaded sample changes nothing.
    pub fn unload(&mut self, id: SampleId) -> Result<(), EngineError> {
        self.project.set_loaded(id, false)?;
        self.resident.remove(id);
        Ok(())
    }

    /// Reads `id` from the project into the resident set, unless it is
    /// already there.
    fn make_resident(&mut self, id: SampleId) -> Result<(), EngineError> {
        if self.resident.contains(id) {
            return Ok(());
        }
        let audio = self.project.audio(id)?;
        self.resident.insert(id, audio)?;
        Ok(())
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
        with_budget(usize::MAX)
    }

    fn with_budget(budget: usize) -> Engine {
        Engine::on(memory_project(), ResidentSet::new(budget)).unwrap()
    }

    #[test]
    fn import_creates_a_root() {
        let mut e = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert_eq!(e.project().sample(id).unwrap().parent, None);
        assert_eq!(e.project().audio(id).unwrap(), ramp(4));
    }

    #[test]
    fn import_does_not_load() {
        let mut e = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.resident().contains(id));
    }

    #[test]
    fn load_makes_audio_resident_and_sets_the_flag() {
        let mut e = engine();
        let id = e.import(&ramp(4)).unwrap();

        e.load(id).unwrap();
        assert!(e.project().sample(id).unwrap().loaded);
        assert_eq!(e.resident().get(id).unwrap().samples(), &[0, 1, 2, 3]);
        assert_eq!(e.resident().bytes(), ramp(4).bytes());

        // Loading again changes nothing.
        e.load(id).unwrap();
        assert_eq!(e.resident().bytes(), ramp(4).bytes());
    }

    #[test]
    fn unload_clears_the_flag_and_drops_the_audio() {
        let mut e = engine();
        let id = e.import(&ramp(4)).unwrap();
        e.load(id).unwrap();

        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.resident().contains(id));
        assert_eq!(e.resident().bytes(), 0);

        // Unloading again changes nothing.
        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
    }

    #[test]
    fn load_over_budget_is_refused_and_leaves_the_flag_clear() {
        let mut e = with_budget(ramp(4).bytes());
        let a = e.import(&ramp(4)).unwrap();
        let b = e.import(&ramp(4)).unwrap();
        e.load(a).unwrap();

        assert!(matches!(
            e.load(b),
            Err(EngineError::Playback(PlaybackError::OverBudget { .. }))
        ));
        assert!(!e.project().sample(b).unwrap().loaded);
        assert!(!e.resident().contains(b));
        assert!(e.project().sample(a).unwrap().loaded);
    }

    #[test]
    fn load_of_an_unknown_sample_is_refused() {
        let mut e = engine();
        let root = e.import(&ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.load(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert!(matches!(
            e.unload(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert!(!e.resident().contains(bogus));
    }

    #[test]
    fn open_brings_loaded_samples_back_into_ram() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path()).unwrap();
        let kept = e.import(&ramp(4)).unwrap();
        let dropped = e.import(&ramp(8)).unwrap();
        e.load(kept).unwrap();
        e.load(dropped).unwrap();
        e.unload(dropped).unwrap();
        drop(e);

        let e = Engine::open(dir.path()).unwrap();
        assert!(e.resident().contains(kept));
        assert!(!e.resident().contains(dropped));
        assert_eq!(e.resident().bytes(), ramp(4).bytes());
    }

    #[test]
    fn open_fails_when_loaded_samples_no_longer_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path()).unwrap();
        let id = e.import(&ramp(4)).unwrap();
        e.load(id).unwrap();
        drop(e);

        let result = Engine::open_with_budget(dir.path(), ramp(4).bytes() - 1);
        assert!(matches!(
            result,
            Err(EngineError::Playback(PlaybackError::OverBudget { .. }))
        ));
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
