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
use crate::playback::{Playback, PlaybackError, ResidentSet, VoiceId};
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
    playback: Playback,
}

impl Engine {
    /// An engine on the project at `dir`, created there if none exists,
    /// playing through the default audio device.
    ///
    /// Every sample the project marks loaded is brought into RAM before
    /// this returns, so the device comes back ready. If they no longer fit
    /// the budget (it shrank, or the project moved to a smaller device)
    /// the open fails rather than deciding which ones to leave out.
    pub fn open(dir: &Path) -> Result<Self, EngineError> {
        let storage = storage::open(dir)?;
        let playback = Playback::start(SAMPLE_RAM_BUDGET)?;
        Self::on(Project::new(storage), playback)
    }

    /// An engine on an already open project and playback, loading what the
    /// project marks loaded.
    pub(crate) fn on(project: Project, playback: Playback) -> Result<Self, EngineError> {
        let mut engine = Self { project, playback };
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
        self.playback.resident()
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
        self.playback.unload(id);
        Ok(())
    }

    /// Starts playing `id` from its first frame and returns the voice, so
    /// it can be stopped.
    ///
    /// The sample must be loaded; nothing is read from disk here, so a
    /// press costs the same every time. Sound starts at the renderer's next
    /// period. A play never fails for want of a free voice: the renderer
    /// steals the oldest.
    pub fn play(&mut self, id: SampleId) -> Result<VoiceId, EngineError> {
        match self.playback.play(id) {
            Err(PlaybackError::NotLoaded(_)) => {
                // Unknown and unloaded are different mistakes; say which.
                self.project.sample(id)?;
                Err(PlaybackError::NotLoaded(id).into())
            }
            result => Ok(result?),
        }
    }

    /// Stops `voice` if it is still playing. A voice that has finished or
    /// been stolen is left alone.
    pub fn stop(&mut self, voice: VoiceId) -> Result<(), EngineError> {
        Ok(self.playback.stop(voice)?)
    }

    /// Stops every voice.
    pub fn stop_all(&mut self) -> Result<(), EngineError> {
        Ok(self.playback.stop_all()?)
    }

    /// Reads `id` from the project into RAM, unless it is already there.
    fn make_resident(&mut self, id: SampleId) -> Result<(), EngineError> {
        if self.playback.resident().contains(id) {
            return Ok(());
        }
        let audio = self.project.audio(id)?;
        self.playback.load(id, audio)?;
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
    use crate::playback::Renderer;
    use crate::project::tests::memory_project;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    /// An engine on a memory project, and the renderer it commands, which
    /// the test drives by hand.
    fn engine() -> (Engine, Renderer) {
        with_budget(usize::MAX)
    }

    fn with_budget(budget: usize) -> (Engine, Renderer) {
        let (playback, renderer) = Playback::unplugged(budget);
        (Engine::on(memory_project(), playback).unwrap(), renderer)
    }

    /// An engine on the project at `dir` with no audio device, as
    /// `Engine::open` would build it on a machine without one.
    fn on_disk(dir: &Path, budget: usize) -> Engine {
        let project = Project::new(storage::open(dir).unwrap());
        Engine::on(project, Playback::unplugged(budget).0).unwrap()
    }

    /// Renders one period of `frames` frames and returns it.
    fn period(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        r.render(&mut out);
        out
    }

    #[test]
    fn import_creates_a_root() {
        let (mut e, _r) = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert_eq!(e.project().sample(id).unwrap().parent, None);
        assert_eq!(e.project().audio(id).unwrap(), ramp(4));
    }

    #[test]
    fn import_does_not_load() {
        let (mut e, _r) = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.resident().contains(id));
    }

    #[test]
    fn load_makes_audio_resident_and_sets_the_flag() {
        let (mut e, _r) = engine();
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
        let (mut e, _r) = engine();
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
        let (mut e, _r) = with_budget(ramp(4).bytes());
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
        let (mut e, _r) = engine();
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
    fn play_is_heard_from_the_renderers_next_period() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(2)).unwrap();
        e.load(id).unwrap();

        let voice = e.play(id).unwrap();
        let out = period(&mut r, 2);
        // Two frames fit in one two-frame period, so the voice is done.
        assert!(!r.is_playing(voice));
        let expected: Vec<f32> = ramp(2).frames_f32(0..2).flat_map(|s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn each_play_is_its_own_voice() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(100)).unwrap();
        e.load(id).unwrap();

        let a = e.play(id).unwrap();
        let b = e.play(id).unwrap();
        assert_ne!(a, b);
        period(&mut r, 4);
        assert_eq!(r.playing(), 2);

        e.stop(a).unwrap();
        period(&mut r, 4);
        assert!(!r.is_playing(a));
        assert!(r.is_playing(b));

        e.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn play_of_an_unloaded_sample_is_refused() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert!(matches!(
            e.play(id),
            Err(EngineError::Playback(PlaybackError::NotLoaded(s))) if s == id
        ));
        period(&mut r, 4);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn play_of_an_unknown_sample_is_refused_as_unknown() {
        let (mut e, _r) = engine();
        let root = e.import(&ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.play(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(s))) if s == bogus
        ));
    }

    #[test]
    fn open_brings_loaded_samples_back_into_ram() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = on_disk(dir.path(), usize::MAX);
        let kept = e.import(&ramp(4)).unwrap();
        let dropped = e.import(&ramp(8)).unwrap();
        e.load(kept).unwrap();
        e.load(dropped).unwrap();
        e.unload(dropped).unwrap();
        drop(e);

        let e = on_disk(dir.path(), usize::MAX);
        assert!(e.resident().contains(kept));
        assert!(!e.resident().contains(dropped));
        assert_eq!(e.resident().bytes(), ramp(4).bytes());
    }

    #[test]
    fn open_fails_when_loaded_samples_no_longer_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = on_disk(dir.path(), usize::MAX);
        let id = e.import(&ramp(4)).unwrap();
        e.load(id).unwrap();
        drop(e);

        let project = Project::new(storage::open(dir.path()).unwrap());
        let result = Engine::on(project, Playback::unplugged(ramp(4).bytes() - 1).0);
        assert!(matches!(
            result,
            Err(EngineError::Playback(PlaybackError::OverBudget { .. }))
        ));
    }

    #[test]
    fn print_adds_a_child_with_the_effect_applied() {
        let (mut e, _r) = engine();
        let root = e.import(&ramp(4)).unwrap();
        let child = e.print(root, reverse).unwrap();

        assert_eq!(e.project().sample(child).unwrap().parent, Some(root));
        assert_eq!(e.project().audio(child).unwrap().samples(), &[3, 2, 1, 0]);
        // The source is untouched.
        assert_eq!(e.project().audio(root).unwrap().samples(), &[0, 1, 2, 3]);
    }

    #[test]
    fn printing_twice_from_one_source_branches() {
        let (mut e, _r) = engine();
        let root = e.import(&ramp(4)).unwrap();
        let a = e.print(root, reverse).unwrap();
        let b = e.print(root, reverse).unwrap();
        assert_ne!(a, b);
        assert_eq!(e.project().sample(a).unwrap().parent, Some(root));
        assert_eq!(e.project().sample(b).unwrap().parent, Some(root));
    }

    #[test]
    fn print_from_unknown_source_adds_nothing() {
        let (mut e, _r) = engine();
        let root = e.import(&ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.print(bogus, reverse),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert!(e.project().sample(bogus).is_err());
    }
}
