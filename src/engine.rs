//! The engine: the one thing a caller holds, and the only way in.
//!
//! Every role in the crate knows one thing. The engine is where an action
//! that needs two of them is written: an import decodes a file and adds
//! the result to the project, a load reads the project and brings the
//! audio into RAM, a play looks for it there and, on failure, asks the
//! project why. Nothing
//! below the engine is reachable from outside the crate except read-only,
//! so there is one path to every action and the rules hold on it.
//!
//! The engine is also the control side of sound. It owns the memory budget
//! (`Ledger`), and every reservation against it is made here: whatever
//! else holds memory was handed its reservation by the engine. It owns the
//! loaded audio and the sending end of the command ring; the other end
//! is the [`Renderer`] it mints as its output, which turns commands into
//! frames wherever the caller drives it. Nothing the engine does can block that: every
//! change to what is heard is a command pushed on the ring, never a call
//! into the render side. The engine never touches a device; whoever has
//! one hands it the output.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::COMMAND_CAPACITY;
use crate::convert::{self, ConvertError};
use crate::decode::{self, DecodeError};
use crate::project::{Project, ProjectError, SampleId};
use crate::record::Recorder;
use crate::render::{BufferAddr, Command, Renderer, VoiceId};
use crate::storage::{self, StorageError};

use crate::ledger::{Ledger, OverBudget, Reservation};

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    OverBudget(#[from] OverBudget),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Convert(#[from] ConvertError),
    /// The sample exists but is not loaded, so there is nothing in RAM
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
    /// The memory budget; everything the engine holds in RAM is reserved
    /// here first.
    ledger: Ledger,
    /// The loaded samples: their audio in RAM, each with the reservation
    /// that paid for it. This is the engine's cache in front of storage
    /// and the control side's answer to "which samples are in RAM". Every
    /// entry is immutable behind an `Arc` so a voice can hold it while it
    /// plays; the render side only ever sees the clones it was handed.
    /// The memory itself is ordinary heap allocation: the guarantee that
    /// the budget's worth of RAM exists is the operating system's job (a
    /// reservation for the engine's process on the device), not the
    /// engine's.
    loaded: HashMap<SampleId, (Arc<AudioBuffer>, Reservation)>,
    /// The sending end of the command ring; the renderer has the other.
    commands: Producer<Command>,
    /// The next voice id to mint. Only goes up, and this is the one
    /// sender, so an id is never reused.
    next_voice: u64,
    /// The render side, until the caller takes it with [`Engine::output`].
    output: Option<Renderer>,
}

impl Engine {
    /// An engine on the project at `dir`, created there if none exists,
    /// with `budget` bytes of RAM for everything it holds in memory.
    ///
    /// The budget is the caller's to know: it depends on the machine's RAM
    /// and on what else runs there, and the engine never reads either.
    /// Playback reads only loaded audio, so everything a pad can trigger
    /// must fit under it. Loading past it is an error, not an eviction.
    ///
    /// No audio device is involved: the engine works the same on a machine
    /// without one. To hear it, take [`Engine::output`] and drive it.
    ///
    /// Every sample the project marks loaded is brought into RAM before
    /// this returns, so the sampler comes back ready. If they no longer fit
    /// the budget (it shrank, or the project moved to a smaller machine)
    /// the open fails rather than deciding which ones to leave out.
    pub fn open(dir: &Path, budget: usize) -> Result<Self, EngineError> {
        Self::on(Project::new(storage::open(dir)?), budget)
    }

    /// An engine on an already open project with `budget` bytes of RAM,
    /// loading what the project marks loaded.
    pub(crate) fn on(project: Project, budget: usize) -> Result<Self, EngineError> {
        let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
        let mut engine = Self {
            project,
            ledger: Ledger::new(budget),
            loaded: HashMap::new(),
            commands: producer,
            next_voice: 0,
            output: Some(Renderer::new(consumer)),
        };
        for id in engine.project.loaded_samples()? {
            engine.load(id)?;
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

    /// A recorder with room for `frames` frames of `channels` channels:
    /// the control side of recording, which the caller keeps.
    ///
    /// The memory is reserved here, against the same figure as everything
    /// else the engine holds, and belongs to the recorder until it is
    /// dropped. A recorder that does not fit beside what is loaded is
    /// refused and nothing changes. The engine keeps no record of
    /// recorders; it meets a recording again only when asked to store it.
    pub fn recorder(&self, channels: u16, frames: usize) -> Result<Recorder, EngineError> {
        let reservation = self
            .ledger
            .reserve(AudioBuffer::bytes_for(channels, frames))?;
        Ok(Recorder::new(reservation))
    }

    /// Imports an audio file as a new root.
    ///
    /// The file is read in whatever format it is in, converted to the
    /// project format (48 kHz, 16-bit, its own channel count up to two)
    /// and stored. The file itself is not kept or referenced: the project
    /// holds its own copy. The new sample is not loaded; that is a separate
    /// decision, so an import lands on disk whether or not it fits in RAM.
    /// The add itself is the project's; a finished recording will make the
    /// same call with its own buffer, and once in, the two are
    /// indistinguishable.
    ///
    /// The buffers in flight count against the budget while they exist:
    /// the decoded samples, reserved from the file's header before any are
    /// read, and what the conversion will allocate, reserved from the
    /// decoded audio before it is converted. A file too large to pass through RAM
    /// beside what is loaded is refused and nothing is stored.
    pub fn import(&mut self, file: &Path) -> Result<SampleId, EngineError> {
        let decoded_reservation = self.ledger.reserve(decode::byte_cost(file)?)?;
        let decoded = decode::decode(file, &decoded_reservation)?;
        let conversion_reservation = self.ledger.reserve(convert::byte_cost(&decoded)?)?;
        let audio = convert::convert(decoded, &conversion_reservation)?;
        // Convert consumed the decoded samples, so their bytes go back now.
        // The conversion's reservation covers `audio` until it is on disk.
        drop(decoded_reservation);
        Ok(self.project.add_sample(None, &audio)?)
    }

    /// Loads `id`: brings its audio into RAM and marks it loaded, so it
    /// stays loaded across a reopen.
    ///
    /// The audio goes into RAM first and the flag is set only if that
    /// succeeded, so the flag is never true for a sample that is not in
    /// RAM. The memory is reserved before the read, from the size the
    /// project records for the sample, so a refused load never allocates.
    /// A sample that does not fit the budget is refused and nothing
    /// changes. Loading a loaded sample changes nothing: the audio is
    /// already here and storage leaves a flag that already has its value
    /// alone, so `on` calls this for every sample the project marks loaded
    /// without writing anything.
    pub fn load(&mut self, id: SampleId) -> Result<(), EngineError> {
        if !self.loaded.contains_key(&id) {
            let sample = self.project.sample(id)?;
            let reservation = self
                .ledger
                .reserve(AudioBuffer::bytes_for(sample.channels, sample.frames))?;
            let audio = self.project.audio(id, &reservation)?;
            self.loaded.insert(id, (Arc::new(audio), reservation));
        }
        self.project.set_loaded(id, true)?;
        Ok(())
    }

    /// Unloads `id`: stops every voice playing it, clears the flag and
    /// drops its audio from RAM. The reservation is released at once.
    ///
    /// Unloaded means gone from RAM, so sound from the sample stops too,
    /// hard, at the output's next render, as `stop` does. Either the whole
    /// unload happens or none of it: a full ring refuses it with
    /// `CommandsFull` before the flag is touched, and a flag that cannot
    /// be written refuses it before the eviction is sent, so a failed
    /// unload never cuts a sound. The eviction goes out before the engine
    /// lets go of the buffer, which is what makes the address a safe key;
    /// see `BufferAddr`. Unloading an unloaded sample changes nothing.
    pub fn unload(&mut self, id: SampleId) -> Result<(), EngineError> {
        let Some((audio, _)) = self.loaded.get(&id) else {
            return Ok(self.project.set_loaded(id, false)?);
        };
        // Room is checked first and the push comes last, so the push
        // cannot fail: this is the ring's only producer.
        if self.commands.is_full() {
            return Err(EngineError::CommandsFull);
        }
        let addr = BufferAddr::of(audio);
        self.project.set_loaded(id, false)?;
        self.send(Command::Evict(addr))?;
        self.loaded.remove(&id);
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
        let Some((audio, _)) = self.loaded.get(&id) else {
            // Unknown and unloaded are different mistakes; say which.
            self.project.sample(id)?;
            return Err(EngineError::NotLoaded(id));
        };
        let voice = VoiceId(self.next_voice);
        let audio = Arc::clone(audio);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::tests::{memory_project, project};
    use crate::testing::paid;

    fn ramp(n: i16) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).collect::<Vec<_>>()).unwrap()
    }

    /// The stored audio of `id`, read outside the budget. Audio never
    /// leaves the engine; tests look at it through the project directly.
    fn audio(e: &Engine, id: SampleId) -> AudioBuffer {
        e.project.audio(id, &paid()).unwrap()
    }

    /// An engine on a memory project, and its output, which the test
    /// drives by hand.
    fn engine() -> (Engine, Renderer) {
        let mut engine = Engine::on(memory_project(), usize::MAX).unwrap();
        let output = engine.output().unwrap();
        (engine, output)
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
        let id = e.project.add_sample(None, &ramp(2)).unwrap();
        e.load(id).unwrap();
        e.play(id).unwrap();

        let mut o = e.output().unwrap();
        let out = period(&mut o, 2);
        let expected: Vec<f32> = ramp(2).frames_f32(0..2).flat_map(|s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    fn wav(dir: &Path, name: &str, rate: u32, samples: &[i16]) -> std::path::PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
        path
    }

    #[test]
    fn import_of_a_wav_in_the_project_format_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "a.wav", 48_000, &[0, 1000, -1000, 32_767]);
        let (mut e, _r) = engine();
        let id = e.import(&path).unwrap();
        let audio = audio(&e, id);
        assert_eq!(audio.channels(), 1);
        assert_eq!(audio.frames(), 4);
        // Already the project format: stored byte for byte.
        assert_eq!(audio.samples(), &[0, 1000, -1000, 32_767]);
        assert_eq!(e.project().sample(id).unwrap().parent, None);
    }

    #[test]
    fn import_resamples_a_wav_at_another_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "b.wav", 44_100, &[0; 44_100]);
        let (mut e, _r) = engine();
        let id = e.import(&path).unwrap();
        let frames = audio(&e, id).frames() as i64;
        assert!((frames - 48_000).abs() <= 2, "{frames}");
    }

    #[test]
    fn import_of_a_file_that_is_not_audio_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "not audio").unwrap();
        let (mut e, _r) = engine();
        assert!(matches!(
            e.import(&path),
            Err(EngineError::Decode(DecodeError::UnknownFormat { .. }))
        ));
        assert!(e.project().loaded_samples().unwrap().is_empty());
    }

    #[test]
    fn import_releases_its_working_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "c.wav", 44_100, &[0; 4410]);
        let (mut e, _r) = engine();
        e.import(&path).unwrap();
        assert_eq!(e.ledger.in_use(), 0);
    }

    #[test]
    fn import_that_does_not_fit_beside_loaded_audio_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "d.wav", 48_000, &[0; 100]);
        // Room for the loaded ramp and nothing else.
        let mut e = Engine::on(memory_project(), ramp(4).bytes()).unwrap();
        let root = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(root).unwrap();

        assert!(matches!(e.import(&path), Err(EngineError::OverBudget(_))));
        assert!(e.project().sample(SampleId(root.0 + 1)).is_err());
        assert_eq!(e.ledger.in_use(), ramp(4).bytes());
    }

    #[test]
    fn import_creates_a_root() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        assert_eq!(e.project().sample(id).unwrap().parent, None);
        assert_eq!(audio(&e, id), ramp(4));
    }

    #[test]
    fn import_does_not_load() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.loaded.contains_key(&id));
    }

    #[test]
    fn load_brings_audio_into_ram_and_sets_the_flag() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();

        e.load(id).unwrap();
        assert!(e.project().sample(id).unwrap().loaded);
        assert_eq!(e.loaded[&id].0.samples(), &[0, 1, 2, 3]);
        assert_eq!(e.ledger.in_use(), ramp(4).bytes());

        // Loading again changes nothing.
        e.load(id).unwrap();
        assert_eq!(e.ledger.in_use(), ramp(4).bytes());
    }

    #[test]
    fn unload_clears_the_flag_and_drops_the_audio() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(id).unwrap();

        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
        assert!(!e.loaded.contains_key(&id));
        assert_eq!(e.ledger.in_use(), 0);

        // Unloading again changes nothing.
        e.unload(id).unwrap();
        assert!(!e.project().sample(id).unwrap().loaded);
    }

    #[test]
    fn load_over_budget_is_refused_and_leaves_the_flag_clear() {
        let mut e = Engine::on(memory_project(), ramp(4).bytes()).unwrap();
        let a = e.project.add_sample(None, &ramp(4)).unwrap();
        let b = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(a).unwrap();

        assert!(matches!(e.load(b), Err(EngineError::OverBudget(_))));
        assert!(!e.project().sample(b).unwrap().loaded);
        assert!(!e.loaded.contains_key(&b));
        assert!(e.project().sample(a).unwrap().loaded);
    }

    #[test]
    fn a_sample_whose_wav_went_bad_can_still_be_unloaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(id).unwrap();
        std::fs::remove_file(dir.path().join("samples").join(format!("{}.wav", id.0))).unwrap();

        // The record cannot be read without the file; existence still can,
        // so the audio and its reservation are not stuck in RAM.
        assert!(e.project().sample(id).is_err());
        e.unload(id).unwrap();
        assert!(!e.loaded.contains_key(&id));
        assert_eq!(e.ledger.in_use(), 0);
    }

    #[test]
    fn load_over_budget_reserves_before_it_reads() {
        let (project, state) = project();
        let mut e = Engine::on(project, ramp(4).bytes() - 1).unwrap();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        state.borrow_mut().audio_reads = 0;

        assert!(matches!(e.load(id), Err(EngineError::OverBudget(_))));
        // Refused on the record alone: the audio was never brought in.
        assert_eq!(state.borrow().audio_reads, 0);
        assert_eq!(e.ledger.in_use(), 0);
    }

    #[test]
    fn load_of_an_unknown_sample_is_refused() {
        let (mut e, _r) = engine();
        let root = e.project.add_sample(None, &ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.load(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert!(matches!(
            e.unload(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(id))) if id == bogus
        ));
        assert!(!e.loaded.contains_key(&bogus));
    }

    #[test]
    fn play_is_heard_from_the_renderers_next_period() {
        let (mut e, mut r) = engine();
        let id = e.project.add_sample(None, &ramp(2)).unwrap();
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
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
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
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
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
    fn unload_stops_the_voices_playing_the_sample() {
        let (mut e, mut r) = engine();
        let gone = e.project.add_sample(None, &ramp(100)).unwrap();
        let kept = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(gone).unwrap();
        e.load(kept).unwrap();
        let a = e.play(gone).unwrap();
        let b = e.play(kept).unwrap();
        period(&mut r, 4);

        e.unload(gone).unwrap();
        // Gone from the output at its next render, and only that sample.
        period(&mut r, 4);
        assert!(!r.is_playing(a));
        assert!(r.is_playing(b));
    }

    #[test]
    fn unload_on_a_full_ring_is_refused_and_leaves_the_sample_loaded() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(id).unwrap();
        for _ in 0..COMMAND_CAPACITY {
            e.play(id).unwrap();
        }

        assert!(matches!(e.unload(id), Err(EngineError::CommandsFull)));
        assert!(e.loaded.contains_key(&id));
        assert!(e.project().sample(id).unwrap().loaded);
        assert_eq!(e.ledger.in_use(), ramp(100).bytes());
    }

    #[test]
    #[cfg(unix)]
    fn unload_whose_flag_cannot_be_written_sends_no_eviction() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let mut r = e.output().unwrap();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(id).unwrap();
        let voice = e.play(id).unwrap();
        period(&mut r, 4);
        // A read-only directory: the manifest cannot be rewritten.
        let perms = |mode| {
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap()
        };
        perms(0o555);
        let result = e.unload(id);
        perms(0o755);

        assert!(matches!(
            result,
            Err(EngineError::Project(ProjectError::Storage(_)))
        ));
        // The sample is still in RAM and the sound was not cut.
        assert!(e.loaded.contains_key(&id));
        assert!(e.project().sample(id).unwrap().loaded);
        period(&mut r, 4);
        assert!(r.is_playing(voice));
    }

    #[test]
    fn unload_of_a_sample_not_in_ram_sends_nothing() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.unload(id).unwrap();
        // The ring still has every slot: nothing was pushed.
        let mut free = 0;
        while e.stop_all().is_ok() {
            free += 1;
        }
        assert_eq!(free, COMMAND_CAPACITY);
        // And with the ring full, the same unload needs no room on it.
        e.unload(id).unwrap();
    }

    #[test]
    fn play_of_an_unloaded_sample_is_refused() {
        let (mut e, mut r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
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
        let root = e.project.add_sample(None, &ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            e.play(bogus),
            Err(EngineError::Project(ProjectError::UnknownSample(s))) if s == bogus
        ));
    }

    #[test]
    fn open_brings_loaded_samples_back_into_ram() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let kept = e.project.add_sample(None, &ramp(4)).unwrap();
        let dropped = e.project.add_sample(None, &ramp(8)).unwrap();
        e.load(kept).unwrap();
        e.load(dropped).unwrap();
        e.unload(dropped).unwrap();
        drop(e);

        let e = Engine::open(dir.path(), usize::MAX).unwrap();
        assert!(e.loaded.contains_key(&kept));
        assert!(!e.loaded.contains_key(&dropped));
        assert_eq!(e.ledger.in_use(), ramp(4).bytes());
    }

    #[test]
    fn open_fails_when_loaded_samples_no_longer_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(id).unwrap();
        drop(e);

        let result = Engine::open(dir.path(), ramp(4).bytes() - 1);
        assert!(matches!(result, Err(EngineError::OverBudget(_))));
    }
    #[test]
    fn a_recorder_pays_for_its_memory_when_it_is_made() {
        let cost = AudioBuffer::bytes_for(2, 1000);
        let e = Engine::on(memory_project(), cost).unwrap();
        let r = e.recorder(2, 1000).unwrap();
        assert_eq!(e.ledger.in_use(), cost);
        assert!(matches!(e.recorder(1, 1), Err(EngineError::OverBudget(_))));
        drop(r);
        assert_eq!(e.ledger.in_use(), 0);
    }
}
