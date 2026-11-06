//! The engine: the one thing a caller holds, and the only way in.
//!
//! Every role in the crate knows one thing. The engine is where an action
//! that needs two of them is written: a print reads the project and runs
//! an effect, a load reads the project and fills the resident set, a play
//! checks the resident set and, on failure, asks the project why. Nothing
//! below the engine is reachable from outside the crate except read-only,
//! so there is one path to every action and the rules hold on it.
//!
//! The engine is also the control side of sound. It owns the audio in RAM
//! (`ResidentSet`) and the sending end of the command ring; the other end
//! is the [`Renderer`] it mints as its output, which turns commands into
//! frames wherever the caller drives it. Nothing the engine does can block that: every
//! change to what is heard is a command pushed on the ring, never a call
//! into the render side. The engine never touches a device; whoever has
//! one hands it the output.

pub(crate) mod resident;

use std::path::Path;

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::{COMMAND_CAPACITY, SAMPLE_RAM_BUDGET};
use crate::project::{Project, ProjectError, SampleId};
use crate::render::{Command, Renderer, VoiceId};
use crate::storage::{self, StorageError};

pub use resident::OverBudget;
use resident::ResidentSet;

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    OverBudget(#[from] OverBudget),
    /// The sample exists but is not loaded, so there is nothing resident
    /// to play. Load it first.
    #[error("sample {} is not loaded", .0.0)]
    NotLoaded(SampleId),
    /// The command ring was full; the command was dropped.
    #[error("the output has not taken the last {COMMAND_CAPACITY} commands")]
    CommandsFull,
}

/// The engine for one open project.
pub struct Engine {
    project: Project,
    /// Audio in RAM, under the budget.
    resident: ResidentSet,
    /// The sending end of the command ring; the renderer has the other.
    commands: Producer<Command>,
    /// The next voice id to mint. Only goes up, and this is the one
    /// sender, so an id is never reused.
    next_voice: u64,
    /// The render side, until the caller takes it with [`Engine::output`].
    output: Option<Renderer>,
}

impl Engine {
    /// An engine on the project at `dir`, created there if none exists.
    ///
    /// No audio device is involved: the engine works the same on a machine
    /// without one. To hear it, take [`Engine::output`] and drive it.
    ///
    /// Every sample the project marks loaded is brought into RAM before
    /// this returns, so the sampler comes back ready. If they no longer fit
    /// the budget (it shrank, or the project moved to a smaller machine)
    /// the open fails rather than deciding which ones to leave out.
    pub fn open(dir: &Path) -> Result<Self, EngineError> {
        Self::on(Project::new(storage::open(dir)?), SAMPLE_RAM_BUDGET)
    }

    /// An engine on an already open project with `budget` bytes for
    /// resident audio, loading what the project marks loaded.
    pub(crate) fn on(project: Project, budget: usize) -> Result<Self, EngineError> {
        let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
        let mut engine = Self {
            project,
            resident: ResidentSet::new(budget),
            commands: producer,
            next_voice: 0,
            output: Some(Renderer::new(consumer)),
        };
        for id in engine.project.loaded_samples()? {
            engine.make_resident(id)?;
        }
        Ok(engine)
    }

    /// The engine's output: its render side, to be driven by whoever owns
    /// the audio thread, moved into a device callback for live sound or
    /// called in a loop offline. There is one; the second call returns
    /// `None`.
    ///
    /// Commands sent before this is taken are not lost. They wait in the
    /// ring and apply on the first render.
    pub fn output(&mut self) -> Option<Renderer> {
        self.output.take()
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
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

    /// Starts playing `id` from its first frame and returns the voice, so
    /// it can be stopped.
    ///
    /// The sample must be loaded; nothing is read from disk here, so a
    /// press costs the same every time. Sound starts at the output's next
    /// render. A play never fails for want of a free voice: the renderer
    /// steals the oldest.
    pub fn play(&mut self, id: SampleId) -> Result<VoiceId, EngineError> {
        let Some(audio) = self.resident.get(id) else {
            // Unknown and unloaded are different mistakes; say which.
            self.project.sample(id)?;
            return Err(EngineError::NotLoaded(id));
        };
        let voice = VoiceId(self.next_voice);
        self.send(Command::Play { id: voice, audio })?;
        // Minted only once the command is in; a refused press burns no id.
        self.next_voice += 1;
        Ok(voice)
    }

    /// Stops `voice` if it is still playing. A voice that has finished or
    /// been stolen is left alone.
    pub fn stop(&mut self, voice: VoiceId) -> Result<(), EngineError> {
        self.send(Command::Stop(voice))
    }

    /// Stops every voice.
    pub fn stop_all(&mut self) -> Result<(), EngineError> {
        self.send(Command::StopAll)
    }

    /// Pushes `command` to the output. A full ring drops it and says so.
    fn send(&mut self, command: Command) -> Result<(), EngineError> {
        self.commands
            .push(command)
            .map_err(|_| EngineError::CommandsFull)
    }

    /// Reads `id` from the project into RAM, unless it is already there.
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

    /// An engine on a memory project, and its output, which the test
    /// drives by hand.
    fn engine() -> (Engine, Renderer) {
        let mut engine = Engine::on(memory_project(), usize::MAX).unwrap();
        let output = engine.output().unwrap();
        (engine, output)
    }

    /// An engine on the project at `dir`, as `Engine::open` builds it.
    fn on_disk(dir: &Path) -> Engine {
        Engine::on(Project::new(storage::open(dir).unwrap()), usize::MAX).unwrap()
    }

    /// Renders one period of `frames` frames and returns it.
    fn period(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        r.render(&mut out);
        out
    }

    #[test]
    fn output_is_taken_once() {
        let mut e = Engine::on(memory_project(), usize::MAX).unwrap();
        assert!(e.output().is_some());
        assert!(e.output().is_none());
    }

    #[test]
    fn commands_sent_before_the_output_is_taken_wait_for_it() {
        let mut e = Engine::on(memory_project(), usize::MAX).unwrap();
        let id = e.import(&ramp(2)).unwrap();
        e.load(id).unwrap();
        e.play(id).unwrap();

        let mut o = e.output().unwrap();
        let out = period(&mut o, 2);
        let expected: Vec<f32> = ramp(2).frames_f32(0..2).flat_map(|s| [s, s]).collect();
        assert_eq!(out, expected);
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
        assert!(!e.resident.contains(id));
    }

    #[test]
    fn load_makes_audio_resident_and_sets_the_flag() {
        let (mut e, _r) = engine();
        let id = e.import(&ramp(4)).unwrap();

        e.load(id).unwrap();
        assert!(e.project().sample(id).unwrap().loaded);
        assert_eq!(e.resident.get(id).unwrap().samples(), &[0, 1, 2, 3]);
        assert_eq!(e.resident.bytes(), ramp(4).bytes());

        // Loading again changes nothing.
        e.load(id).unwrap();
        assert_eq!(e.resident.bytes(), ramp(4).bytes());
    }

    #[test]
    fn unload_clears_the_flag_and_drops_the_audio() {
        let (mut e, _r) = engine();
        let id = e.import(&ramp(4)).unwrap();
        e.load(id).unwrap();

        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.resident.contains(id));
        assert_eq!(e.resident.bytes(), 0);

        // Unloading again changes nothing.
        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
    }

    #[test]
    fn load_over_budget_is_refused_and_leaves_the_flag_clear() {
        let mut e = Engine::on(memory_project(), ramp(4).bytes()).unwrap();
        let a = e.import(&ramp(4)).unwrap();
        let b = e.import(&ramp(4)).unwrap();
        e.load(a).unwrap();

        assert!(matches!(e.load(b), Err(EngineError::OverBudget(_))));
        assert!(!e.project().sample(b).unwrap().loaded);
        assert!(!e.resident.contains(b));
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
        assert!(!e.resident.contains(bogus));
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
    fn a_full_ring_refuses_the_command_and_burns_no_voice_id() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(100)).unwrap();
        e.load(id).unwrap();
        let mut last = None;
        for _ in 0..COMMAND_CAPACITY {
            last = Some(e.play(id).unwrap());
        }
        assert!(matches!(e.play(id), Err(EngineError::CommandsFull)));
        assert!(matches!(e.stop_all(), Err(EngineError::CommandsFull)));

        // Once the renderer has drained the ring, the next voice id is the
        // one after the last accepted play, not after the refused ones.
        period(&mut r, 4);
        let next = e.play(id).unwrap();
        assert_eq!(next.0, last.unwrap().0 + 1);
    }

    #[test]
    fn unload_keeps_a_playing_voice_alive() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(100)).unwrap();
        e.load(id).unwrap();
        let voice = e.play(id).unwrap();
        period(&mut r, 4);

        e.unload(id).unwrap();
        assert!(!e.resident.contains(id));
        period(&mut r, 4);
        assert!(r.is_playing(voice));
    }

    #[test]
    fn play_of_an_unloaded_sample_is_refused() {
        let (mut e, mut r) = engine();
        let id = e.import(&ramp(4)).unwrap();
        assert!(matches!(
            e.play(id),
            Err(EngineError::NotLoaded(s)) if s == id
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
        let mut e = on_disk(dir.path());
        let kept = e.import(&ramp(4)).unwrap();
        let dropped = e.import(&ramp(8)).unwrap();
        e.load(kept).unwrap();
        e.load(dropped).unwrap();
        e.unload(dropped).unwrap();
        drop(e);

        let e = on_disk(dir.path());
        assert!(e.resident.contains(kept));
        assert!(!e.resident.contains(dropped));
        assert_eq!(e.resident.bytes(), ramp(4).bytes());
    }

    #[test]
    fn open_fails_when_loaded_samples_no_longer_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = on_disk(dir.path());
        let id = e.import(&ramp(4)).unwrap();
        e.load(id).unwrap();
        drop(e);

        let result = Engine::on(
            Project::new(storage::open(dir.path()).unwrap()),
            ramp(4).bytes() - 1,
        );
        assert!(matches!(result, Err(EngineError::OverBudget(_))));
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
