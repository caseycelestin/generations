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
//! (`Ledger`), and every block allocated against it is allocated by code
//! the engine handed the ledger to: whatever else holds memory was given
//! the right to by the engine. It owns the
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
use crate::config::{COMMAND_CAPACITY, MAX_INPUT_CHANNELS, MAX_INPUT_SOUNDS, MAX_PERIOD_FRAMES};
use crate::dsp::{BufferAddr, SoundId};
use crate::import::{self as import, ImportError};
use crate::input::{self, Channels, Input, InputId};
use crate::memory::Ledger;
use crate::project::{Project, ProjectError, SampleId};
use crate::renderer::Renderer;
use crate::renderer::command::{Command, InputCommand, SampleCommand};
use crate::storage::{self as storage, StorageError};

/// Reasons an engine operation can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Import(#[from] ImportError),
    /// The sample exists but is not loaded, so there is nothing in RAM
    /// to play. Load it first.
    #[error("sample {} is not loaded", .0.0)]
    NotLoaded(SampleId),
    /// The command ring was full; the command was dropped.
    #[error("the output has not taken the last {COMMAND_CAPACITY} commands")]
    CommandsFull,
    /// The layout asked of `Engine::input` adds up to no channels, or
    /// more than the engine takes from one device.
    #[error("an input is 1 to {MAX_INPUT_CHANNELS} channels wide, not {0}")]
    BadInputWidth(usize),
    /// The capture period given to `Engine::input` is zero or longer
    /// than the engine takes.
    #[error("a capture period is 1 to {MAX_PERIOD_FRAMES} frames, not {0}")]
    BadInputPeriod(usize),
    /// The input's sources would not all fit beside the input sounds the
    /// engine already holds. Remove an input before adding another.
    #[error("the engine holds {MAX_INPUT_SOUNDS} input sounds at most")]
    InputsFull,
    /// The sound is not a source of an input the engine holds: a sample,
    /// a source of an input already removed, or one it never minted.
    #[error("sound {} is not an input source", .0.0)]
    NotAnInputSource(SoundId),
    /// The input is not one the engine holds: already removed, or never
    /// minted.
    #[error("input {} is not held", .0.0)]
    NoSuchInput(InputId),
}

/// The engine for one open project.
pub struct Engine {
    project: Project,
    /// The memory budget; every buffer the engine holds in RAM is a block
    /// allocated from it, and repays it when dropped.
    ledger: Arc<Ledger>,
    /// The loaded samples: their audio in RAM. This is the engine's cache
    /// in front of storage and the control side's answer to "which samples
    /// are in RAM". Every entry is immutable behind an `Arc` so a sound
    /// can hold it while it plays; the render side only ever sees the
    /// clones it was handed. The memory itself is ordinary heap
    /// allocation: the guarantee that the budget's worth of RAM exists is
    /// the operating system's job (a reservation for the engine's process
    /// on the device), not the engine's.
    loaded: HashMap<SampleId, Arc<AudioBuffer>>,
    /// The sending end of the command ring; the renderer has the other.
    commands: Producer<Command>,
    /// The next sound id to mint. Only goes up, and this is the one
    /// sender, so an id is never reused.
    next_sound: u64,
    /// The next input id to mint; the same rule.
    next_input: u64,
    /// The inputs added and not yet removed, with the sounds of their
    /// sources. The control side's answer to "is this an input", "is this
    /// a source" and "is there room", so none of those is ever put to the
    /// render side.
    inputs: HashMap<InputId, Vec<SoundId>>,
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
            next_sound: 0,
            next_input: 0,
            inputs: HashMap::new(),
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

    /// Adds an input: the ability to put audio into the renderer. Mints a
    /// ring, sends its reading end to the render side, and returns the
    /// capture end for whoever owns the capture thread to feed from a
    /// device's input callback. Nothing is heard until
    /// [`Engine::monitor`].
    ///
    /// An input is shaped like the device it comes from. `layout` says
    /// what each channel of the device's interleaved frames means, in
    /// order: a `Stereo` is the next two channels heard left and right, a
    /// `Mono` is the next one heard on both sides. The frames fed are as
    /// wide as the layout adds up to, 1 to `MAX_INPUT_CHANNELS`, exactly
    /// as the device delivered them. Each source is a sound of its own,
    /// [`Input::sounds`] in layout order, which `monitor` and `unmonitor`
    /// take; the input as a whole is [`Input::id`], which `remove_input`
    /// takes. The layout is fixed for the input's life: a device that is
    /// to deliver other channels is closed and reopened, and so is its
    /// input. Up to `MAX_INPUT_SOUNDS` sources at once across every input;
    /// an input whose sources would not all fit is refused with
    /// `InputsFull` until one is removed, so an input already heard is
    /// never cut by another being added. Each source is its own ring, split
    /// out of the device frames by [`Input::feed`], so the render side
    /// sees mono and stereo sounds and nothing wider.
    ///
    /// `period` is how many frames the capture device delivers per
    /// callback, which the caller knows from opening it and the engine
    /// cannot: the rings are sized from it, a few periods deep, so the
    /// capture and output clocks have room to disagree before a frame is
    /// dropped. At most `MAX_PERIOD_FRAMES`, which is what makes the rings
    /// fixed overhead with a known worst case rather than audio-sized
    /// memory on the ledger.
    pub fn input(&mut self, layout: &[Channels], period: usize) -> Result<Input, EngineError> {
        let width = input::width(layout);
        if !(1..=MAX_INPUT_CHANNELS).contains(&width) {
            return Err(EngineError::BadInputWidth(width));
        }
        if !(1..=MAX_PERIOD_FRAMES).contains(&period) {
            return Err(EngineError::BadInputPeriod(period));
        }
        let held: usize = self.inputs.values().map(Vec::len).sum();
        if held + layout.len() > MAX_INPUT_SOUNDS {
            return Err(EngineError::InputsFull);
        }
        // One command per source; all of them go or none does.
        if self.commands.slots() < layout.len() {
            return Err(EngineError::CommandsFull);
        }
        // An id for the input and a sound per source.
        let id = InputId(self.next_input);
        let sounds: Vec<SoundId> = (0..layout.len() as u64)
            .map(|n| SoundId(self.next_sound + n))
            .collect();
        // The capture callback writes frames in through the `Input`, the
        // renderer reads them out through the readers on the audio thread.
        let (input, readers) = Input::open(layout, period, id, &sounds);
        for (&id, reader) in sounds.iter().zip(readers) {
            self.send(InputCommand::Add { id, reader })?;
        }
        // Minted only once the commands are in; a refused add burns no id.
        self.next_input += 1;
        self.next_sound += layout.len() as u64;
        self.inputs.insert(id, sounds);
        Ok(input)
    }

    /// Hears the input source `sound`, from [`Input::sounds`], through the
    /// output from the next period. Refused as `NotAnInputSource` for a
    /// sound that is not one.
    pub fn monitor(&mut self, sound: SoundId) -> Result<(), EngineError> {
        self.source_held(sound)?;
        self.send(InputCommand::Monitor(sound))
    }

    /// Stops hearing the input source `sound`. Its input stays and keeps
    /// up with the device; `monitor` hears the source again. Refused as
    /// `NotAnInputSource` for a sound that is not one.
    pub fn unmonitor(&mut self, sound: SoundId) -> Result<(), EngineError> {
        self.source_held(sound)?;
        self.send(InputCommand::Unmonitor(sound))
    }

    /// Removes the input `id`, from [`Input::id`], with every source of it.
    /// Call this before dropping the `Input`: the render side lets go of
    /// its end of each source's ring here, and the rings are then freed
    /// when the `Input` is dropped, on the caller's thread. Dropped the
    /// other way round, the render side holds the last reference to each
    /// and frees them there. Refused as `NoSuchInput` for an input not
    /// held; a refused remove keeps the slots.
    pub fn remove_input(&mut self, id: InputId) -> Result<(), EngineError> {
        let Some(sounds) = self.inputs.get(&id) else {
            return Err(EngineError::NoSuchInput(id));
        };
        // One command per source; all of them go or none does.
        if self.commands.slots() < sounds.len() {
            return Err(EngineError::CommandsFull);
        }
        for &sound in sounds {
            self.commands
                .push(InputCommand::Remove(sound).into())
                .map_err(|_| EngineError::CommandsFull)?;
        }
        self.inputs.remove(&id);
        Ok(())
    }

    /// `Ok` if `sound` is a source of an input the engine holds.
    fn source_held(&self, sound: SoundId) -> Result<(), EngineError> {
        self.inputs
            .values()
            .any(|sources| sources.contains(&sound))
            .then_some(())
            .ok_or(EngineError::NotAnInputSource(sound))
    }

    /// The project this engine operates on.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Imports an audio file as a new root.
    ///
    /// The file is read in whatever format it is in, converted to the
    /// project format and stored. The file itself is not kept or
    /// referenced: the project holds its own copy. The new sample is not
    /// loaded; that is a separate decision, so an import lands on disk
    /// whether or not it fits in RAM. The add itself is the project's; a
    /// finished recording will make the same call with its own buffer,
    /// and once in, the two are indistinguishable.
    ///
    /// The import passes through the budget on its way to disk; a file
    /// too large to pass through RAM beside what is loaded is refused,
    /// and nothing is stored.
    pub fn import(&mut self, file: &Path) -> Result<SampleId, EngineError> {
        let audio = import::import(file, &self.ledger)?;
        Ok(self.project.add_sample(None, &audio)?)
    }

    /// Loads `id`: brings its audio into RAM and marks it loaded, so it
    /// stays loaded across a reopen.
    ///
    /// The audio goes into RAM first and the flag is set only if that
    /// succeeded, so the flag is never true for a sample that is not in
    /// RAM. Storage allocates the block from the budget before it reads
    /// the audio, from the size it stores, so a refused load reads
    /// nothing. A sample that does not fit the budget is refused and
    /// nothing changes. Loading a loaded sample changes nothing: the audio
    /// is already here and storage leaves a flag that already has its
    /// value alone, so `on` calls this for every sample the project marks
    /// loaded without writing anything.
    pub fn load(&mut self, id: SampleId) -> Result<(), EngineError> {
        if !self.loaded.contains_key(&id) {
            let audio = self.project.audio(id, &self.ledger)?;
            // The audio is in RAM before the flag says so, and comes back
            // out if the flag cannot be written, so the two agree.
            self.loaded.insert(id, Arc::new(audio));
            if let Err(e) = self.project.set_loaded(id, true) {
                self.loaded.remove(&id);
                return Err(e.into());
            }
        } else {
            self.project.set_loaded(id, true)?;
        }
        Ok(())
    }

    /// Unloads `id`: stops every sound playing it, clears the flag and
    /// drops its audio from RAM, which repays the budget as soon as the
    /// last sound playing it has let go.
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
        let Some(audio) = self.loaded.get(&id) else {
            return Ok(self.project.set_loaded(id, false)?);
        };
        // Room is checked first and the push comes last, so the push
        // cannot fail: this is the ring's only producer.
        if self.commands.is_full() {
            return Err(EngineError::CommandsFull);
        }
        let addr = BufferAddr::of(audio);
        self.project.set_loaded(id, false)?;
        self.send(SampleCommand::Evict(addr))?;
        self.loaded.remove(&id);
        Ok(())
    }

    /// Starts playing `id` from its first frame and returns the sound, so
    /// it can be stopped.
    ///
    /// The sample must be loaded; nothing is read from disk here, so a
    /// press costs the same every time. Sound starts at the output's next
    /// render. A play never fails for want of a free slot: the renderer
    /// steals the oldest.
    pub fn play(&mut self, id: SampleId) -> Result<SoundId, EngineError> {
        let Some(audio) = self.loaded.get(&id) else {
            // Unknown and unloaded are different mistakes; say which.
            self.project.sample(id)?;
            return Err(EngineError::NotLoaded(id));
        };
        let sound = SoundId(self.next_sound);
        let audio = Arc::clone(audio);
        self.send(SampleCommand::Play { id: sound, audio })?;
        // Minted only once the command is in; a refused press burns no id.
        self.next_sound += 1;
        Ok(sound)
    }

    /// Stops the sample `sound` if it is still playing. One that has
    /// finished or been stolen is left alone. Inputs are untouched: see
    /// [`Engine::unmonitor`] and [`Engine::remove_input`].
    pub fn stop(&mut self, sound: SoundId) -> Result<(), EngineError> {
        self.send(SampleCommand::Stop(sound))
    }

    /// Stops every sample. Inputs keep being heard.
    pub fn stop_all(&mut self) -> Result<(), EngineError> {
        self.send(SampleCommand::StopAll)
    }

    /// Pushes `command` to the output. A full ring drops it and says so.
    fn send(&mut self, command: impl Into<Command>) -> Result<(), EngineError> {
        self.commands
            .push(command.into())
            .map_err(|_| EngineError::CommandsFull)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use crate::config::Pcm;
    use crate::import::DecodeError;
    use crate::project::tests::memory_project;
    use crate::testing::{audio, unlimited};

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        audio(1, &(0..n).map(|i| i as Pcm * 0.01).collect::<Vec<_>>())
    }

    /// What `ramp(n)` costs the budget.
    fn ramp_bytes(n: usize) -> usize {
        n * size_of::<Pcm>()
    }

    /// The stored audio of `id`, read outside the budget. Audio never
    /// leaves the engine; tests look at it through the project directly.
    fn stored(e: &Engine, id: SampleId) -> AudioBuffer {
        e.project.audio(id, &unlimited()).unwrap()
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
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
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
        let audio = stored(&e, id);
        assert_eq!(audio.channels(), 1);
        assert_eq!(audio.frames(), 4);
        // Already the project rate: each sample scaled by its width, no
        // resampling.
        let scaled = |s: i16| Pcm::from(s) / 32_768.0;
        assert_eq!(
            audio.samples(),
            &[scaled(0), scaled(1000), scaled(-1000), scaled(32_767)]
        );
        assert_eq!(e.project().sample(id).unwrap().parent, None);
    }

    #[test]
    fn import_resamples_a_wav_at_another_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "b.wav", 44_100, &[0; 44_100]);
        let (mut e, _r) = engine();
        let id = e.import(&path).unwrap();
        let frames = stored(&e, id).frames() as i64;
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
            Err(EngineError::Import(ImportError::Decode(
                DecodeError::Malformed { .. }
            )))
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
        let mut e = Engine::on(memory_project(), ramp_bytes(4)).unwrap();
        let root = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(root).unwrap();

        assert!(matches!(
            e.import(&path),
            Err(EngineError::Import(ImportError::Decode(
                DecodeError::OverBudget { .. }
            )))
        ));
        assert!(e.project().sample(SampleId(root.0 + 1)).is_err());
        assert_eq!(e.ledger.in_use(), ramp_bytes(4));
    }

    #[test]
    fn import_creates_a_root() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        assert_eq!(e.project().sample(id).unwrap().parent, None);
        assert_eq!(stored(&e, id).samples(), ramp(4).samples());
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
        assert_eq!(e.loaded[&id].samples(), ramp(4).samples());
        assert_eq!(e.ledger.in_use(), ramp_bytes(4));

        // Loading again changes nothing.
        e.load(id).unwrap();
        assert_eq!(e.ledger.in_use(), ramp_bytes(4));
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
        let mut e = Engine::on(memory_project(), ramp_bytes(4)).unwrap();
        let a = e.project.add_sample(None, &ramp(4)).unwrap();
        let b = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(a).unwrap();

        assert!(matches!(
            e.load(b),
            Err(EngineError::Project(ProjectError::Storage(
                StorageError::OverBudget { .. }
            )))
        ));
        assert!(!e.project().sample(b).unwrap().loaded);
        assert!(!e.loaded.contains_key(&b));
        assert!(e.project().sample(a).unwrap().loaded);
        // Refused at the allocation: nothing was brought in and let go.
        assert_eq!(e.ledger.in_use(), ramp_bytes(4));
    }

    #[test]
    fn a_sample_whose_wav_went_bad_can_still_be_unloaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(id).unwrap();
        std::fs::remove_file(dir.path().join("samples").join(format!("{}.wav", id.0))).unwrap();

        // The record comes from the manifest, so the unload goes through
        // and the audio is not stuck in RAM.
        e.unload(id).unwrap();
        assert!(!e.loaded.contains_key(&id));
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

        let play = e.play(id).unwrap();
        let out = period(&mut r, 2);
        // Two frames fit in one two-frame period, so the sound is done.
        assert!(!r.is_playing(play));
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn each_play_is_its_own_sound() {
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
    fn a_full_ring_refuses_the_command_and_burns_no_play_id() {
        let (mut e, mut r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(id).unwrap();
        let mut last = None;
        for _ in 0..COMMAND_CAPACITY {
            last = Some(e.play(id).unwrap());
        }
        assert!(matches!(e.play(id), Err(EngineError::CommandsFull)));
        assert!(matches!(e.stop_all(), Err(EngineError::CommandsFull)));

        // Once the renderer has drained the ring, the next sound id is the
        // one after the last accepted play, not after the refused ones.
        period(&mut r, 4);
        let next = e.play(id).unwrap();
        assert_eq!(next.0, last.unwrap().0 + 1);
    }

    #[test]
    fn an_input_is_one_to_max_channels_wide_and_has_a_period() {
        let (mut e, _r) = engine();
        let too_wide = [Channels::Mono; MAX_INPUT_CHANNELS + 1];
        for bad in [&[][..], &too_wide[..]] {
            assert!(matches!(
                e.input(bad, 64),
                Err(EngineError::BadInputWidth(w)) if w == bad.len()
            ));
        }
        for bad in [0, MAX_PERIOD_FRAMES + 1] {
            assert!(matches!(
                e.input(&[Channels::Mono], bad),
                Err(EngineError::BadInputPeriod(p)) if p == bad
            ));
        }
        assert!(e.inputs.is_empty(), "a refused input holds no slot");
        assert_eq!((e.next_sound, e.next_input), (0, 0), "and burns no id");
    }

    #[test]
    fn each_source_of_an_input_is_heard_on_its_own_and_all_go_when_it_is_removed() {
        let (mut e, mut r) = engine();
        // A device of four channels: a stereo pair, then two microphones.
        let mut input = e
            .input(&[Channels::Stereo, Channels::Mono, Channels::Mono], 64)
            .unwrap();
        let &[pair, mic_a, mic_b] = input.sounds() else {
            panic!("three sources");
        };
        assert_eq!(input.channels(), 4);
        // One four-wide frame per feed, quiet enough that no sum clips.
        let frame = [0.1, 0.2, 0.3, 0.4];

        input.feed(&frame);
        period(&mut r, 1);
        assert!(
            [pair, mic_a, mic_b].iter().all(|&s| r.is_playing(s)),
            "an input's sounds are in the renderer at once"
        );

        e.monitor(pair).unwrap();
        input.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.1, 0.2]);

        e.monitor(mic_b).unwrap();
        e.unmonitor(pair).unwrap();
        input.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.4, 0.4]);

        e.monitor(mic_a).unwrap();
        input.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.3 + 0.4, 0.3 + 0.4]);

        e.remove_input(input.id()).unwrap();
        period(&mut r, 1);
        assert_eq!(r.playing(), 0, "every source of the input is gone");
        assert!(matches!(
            e.remove_input(input.id()),
            Err(EngineError::NoSuchInput(i)) if i == input.id()
        ));
        for gone in [pair, mic_a, mic_b] {
            assert!(matches!(
                e.monitor(gone),
                Err(EngineError::NotAnInputSource(s)) if s == gone
            ));
        }
    }

    #[test]
    fn more_input_sounds_than_slots_are_refused_until_an_input_is_removed() {
        let (mut e, mut r) = engine();
        // One stereo input, then mono inputs up to the slot count.
        let first = e.input(&[Channels::Stereo, Channels::Mono], 64).unwrap();
        let rest: Vec<_> = (2..MAX_INPUT_SOUNDS)
            .map(|_| e.input(&[Channels::Mono], 64).unwrap())
            .collect();
        assert!(matches!(
            e.input(&[Channels::Mono], 64),
            Err(EngineError::InputsFull)
        ));
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);

        // Removing the two-source input makes room for two sources, not three.
        e.remove_input(first.id()).unwrap();
        assert!(matches!(
            e.input(&[Channels::Mono, Channels::Mono, Channels::Mono], 64),
            Err(EngineError::InputsFull)
        ));
        let next = e.input(&[Channels::Mono, Channels::Mono], 64).unwrap();
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);
        assert!(first.sounds().iter().all(|&s| !r.is_playing(s)));
        assert!(next.sounds().iter().all(|&s| r.is_playing(s)));
        assert!(rest.iter().all(|i| r.is_playing(i.sounds()[0])));
    }

    #[test]
    fn a_sample_is_not_an_input_source() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(id).unwrap();
        let play = e.play(id).unwrap();
        for result in [e.monitor(play), e.unmonitor(play)] {
            assert!(matches!(result, Err(EngineError::NotAnInputSource(s)) if s == play));
        }
    }

    #[test]
    fn a_full_ring_refuses_an_input_and_holds_no_slot_and_burns_no_id() {
        let (mut e, mut r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.load(id).unwrap();
        for _ in 0..COMMAND_CAPACITY {
            e.play(id).unwrap();
        }
        let before = e.next_sound;
        assert!(matches!(
            e.input(&[Channels::Stereo], 64),
            Err(EngineError::CommandsFull)
        ));
        assert!(e.inputs.is_empty());
        assert_eq!((e.next_sound, e.next_input), (before, 0));

        period(&mut r, 4);
        let input = e.input(&[Channels::Stereo, Channels::Mono], 64).unwrap();
        assert_eq!(input.id(), InputId(0));
        assert_eq!(input.sounds(), [SoundId(before), SoundId(before + 1)]);
        assert_eq!((e.next_sound, e.next_input), (before + 2, 1));
        assert_eq!(e.inputs[&input.id()], input.sounds());
    }

    #[test]
    fn unload_stops_the_sounds_playing_the_sample() {
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
        assert_eq!(e.ledger.in_use(), ramp_bytes(100));
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
        let play = e.play(id).unwrap();
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
        assert!(r.is_playing(play));
    }

    #[test]
    fn unload_of_a_sample_not_in_ram_sends_nothing() {
        let (mut e, _r) = engine();
        let id = e.project.add_sample(None, &ramp(100)).unwrap();
        e.unload(id).unwrap();
        // The ring still has every sound: nothing was pushed.
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
        assert_eq!(e.ledger.in_use(), ramp_bytes(4));
    }

    #[test]
    fn open_fails_when_loaded_samples_no_longer_fit() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::open(dir.path(), usize::MAX).unwrap();
        let id = e.project.add_sample(None, &ramp(4)).unwrap();
        e.load(id).unwrap();
        drop(e);

        let result = Engine::open(dir.path(), ramp_bytes(4) - 1);
        assert!(matches!(
            result,
            Err(EngineError::Project(ProjectError::Storage(
                StorageError::OverBudget { .. }
            )))
        ));
    }
}
