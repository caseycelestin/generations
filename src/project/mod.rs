//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over a `Storage`, and the one way audio
//! moves between the disk and RAM. It keeps no copy of the data: every
//! read and write goes to storage, so changing the project is changing
//! the disk and there is no save step. Today a project holds samples;
//! patterns and settings arrive as their steps come.
//!
//! The project makes and holds the [`Ledger`], because every byte of
//! audio it brings into RAM, an import on its way to disk or a sample
//! loaded for playback, is a block on it. What it hands out is a
//! [`Sample`], plain audio owned by whoever holds it; the project
//! knows nothing about sound, and the renderer that plays the sample
//! knows nothing about the project. The caller carries the sample from
//! one to the other.

pub(crate) mod import;
pub(crate) mod ledger;
pub(crate) mod storage;

use std::path::Path;

use thiserror::Error;

use crate::audio::{AudioBuffer, Sample, Take};
use crate::project::ledger::Ledger;
use import::ImportError;
use storage::{Storage, StorageError};

/// Identity of a sample within one project.
///
/// A sample does not exist until it is stored, so storage mints the id
/// when it adds one, from a per-project counter. Ids are never reused and
/// are not meaningful across projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Everything stored about one sample except its audio and its id: what
/// storage writes beside the audio when a sample is added. Part of the
/// storage format, so it stays inside the crate.
///
/// Empty for now. Fields join it as steps need them; the parent pointer
/// comes back here with prints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SampleMetadata {}

/// Reasons a project operation can fail.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// No sample with this id exists in the project.
    #[error("no sample with id {}", .0.0)]
    UnknownSample(SampleId),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Import(#[from] ImportError),
}

/// One project, live on its storage, allocating from one ledger.
pub struct Project {
    storage: Box<dyn Storage>,
    /// Where every buffer this project brings into RAM is charged.
    ledger: Ledger,
}

impl Project {
    /// The project at `dir`, created there if none exists, holding at
    /// most `budget` bytes of audio in RAM at once.
    ///
    /// The budget is the caller's, because it is the machine's to know.
    /// Nothing is loaded here: what to load is the caller's to remember.
    ///
    /// `storage::open` creates an empty project if the directory is empty
    /// or missing, validates the whole project it finds otherwise, and
    /// refuses a directory holding anything else. From then on every
    /// change is durable by the time the call that made it returns.
    pub fn open(dir: &Path, budget: usize) -> Result<Self, ProjectError> {
        Ok(Self::on(storage::open(dir)?, budget))
    }

    /// The project in `storage`. A storage that exists holds a whole,
    /// valid project, so there is nothing to check here.
    pub(crate) fn on(storage: Box<dyn Storage>, budget: usize) -> Self {
        Self {
            storage,
            ledger: Ledger::new(budget),
        }
    }

    /// Imports an audio file as a new sample.
    ///
    /// The file is read in whatever format it is in, converted to the
    /// project format and stored. The file itself is not kept or
    /// referenced: the project holds its own copy. The new sample is not
    /// loaded; that is a separate decision, so an import lands on disk
    /// whether or not it fits in RAM. The add itself is
    /// `add_sample`; [`save_take`](Self::save_take) makes the same call
    /// with a take, and once in, the two are indistinguishable.
    ///
    /// The import passes through the budget on its way to disk; a file
    /// too large to pass through RAM beside what is loaded is refused,
    /// and nothing is stored.
    pub fn import(&mut self, file: &Path) -> Result<SampleId, ProjectError> {
        let audio = import::import(file, &self.ledger)?;
        self.add_sample(&audio)
    }

    /// Saves `take` as a new sample and returns its id: the frames it
    /// kept, not the room after them. The take is read where it is, in
    /// its recorder's buffer, and nothing is copied into RAM. The new
    /// sample is not loaded; [`load`](Self::load) it to play it.
    pub fn save_take(&mut self, take: &Take) -> Result<SampleId, ProjectError> {
        self.add_sample(take.audio())
    }

    /// Loads `id`: brings its audio into RAM and returns it as a
    /// [`Sample`], whose holder owns it. Drop it and the sample is
    /// unloaded, its block repaying the budget once the last sound playing
    /// it has let go.
    ///
    /// Storage allocates the block from the budget before it reads the
    /// audio, from the size it stores, so a refused load reads nothing. A
    /// sample that does not fit the budget is refused and nothing changes.
    ///
    /// Each load is its own copy: the project keeps no list of what is in
    /// RAM, so loading a sample twice costs its memory twice. Hold one
    /// per sample.
    pub fn load(&self, id: SampleId) -> Result<Sample, ProjectError> {
        Ok(Sample::new(self.audio(id)?))
    }

    /// Adds audio as a sample and returns its id.
    pub(crate) fn add_sample(&mut self, audio: &AudioBuffer) -> Result<SampleId, ProjectError> {
        Ok(self.storage.add_sample(&SampleMetadata {}, audio)?)
    }

    /// The audio of the sample with this id, read from storage into a
    /// block on the ledger. [`load`](Self::load) is the public face of
    /// it; tests read through here.
    pub(crate) fn audio(&self, id: SampleId) -> Result<AudioBuffer, ProjectError> {
        self.storage
            .audio(id, &self.ledger)?
            .ok_or(ProjectError::UnknownSample(id))
    }

    /// The ledger this project allocates from.
    #[cfg(test)]
    pub(crate) fn ledger(&self) -> &Ledger {
        &self.ledger
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use std::mem::size_of;
    use std::path::Path;

    use super::*;
    use crate::audio::ChannelLayout;
    use crate::config::Pcm;
    use crate::project::import::DecodeError;
    use crate::testing::audio;

    /// What the stub storage remembers.
    #[derive(Default)]
    pub(crate) struct State {
        next_id: u32,
        /// Each sample with its layout and audio as plain samples: the
        /// backend's own copy, outside the budget, as a disk is.
        samples: Vec<(SampleId, ChannelLayout, Vec<Pcm>)>,
    }

    /// In-memory storage for tests, shared with the test through `Rc` so
    /// it can be inspected after the project takes ownership.
    struct Memory(Rc<RefCell<State>>);

    impl Storage for Memory {
        fn has_sample(&self, id: SampleId) -> Result<bool, StorageError> {
            Ok(self.find(id).is_some())
        }

        fn audio(
            &self,
            id: SampleId,
            ledger: &Ledger,
        ) -> Result<Option<AudioBuffer>, StorageError> {
            let Some((_, layout, samples)) = self.find(id) else {
                return Ok(None);
            };
            let mut block = ledger
                .alloc(samples.len())
                .map_err(|source| StorageError::OverBudget { id, source })?;
            block.extend(samples);
            Ok(Some(AudioBuffer::new(layout, block).unwrap()))
        }

        fn add_sample(
            &mut self,
            _metadata: &SampleMetadata,
            audio: &AudioBuffer,
        ) -> Result<SampleId, StorageError> {
            let mut s = self.0.borrow_mut();
            let id = SampleId(s.next_id);
            s.next_id += 1;
            s.samples
                .push((id, audio.layout(), audio.samples().to_vec()));
            Ok(id)
        }
    }

    impl Memory {
        fn find(&self, id: SampleId) -> Option<(SampleId, ChannelLayout, Vec<Pcm>)> {
            self.0
                .borrow()
                .samples
                .iter()
                .find(|(s, _, _)| *s == id)
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
        let (state, storage) = memory_storage();
        (Project::on(storage, usize::MAX), state)
    }

    /// In-memory storage and its state, for a project on a ledger of the
    /// test's choosing.
    fn memory_storage() -> (Rc<RefCell<State>>, Box<dyn Storage>) {
        let state = Rc::new(RefCell::new(State::default()));
        (state.clone(), Box::new(Memory(state)))
    }

    fn tone() -> AudioBuffer {
        audio(ChannelLayout::Mono, &[0.0, 0.1, 0.2, 0.3])
    }

    fn structure(state: &Rc<RefCell<State>>) -> Vec<SampleId> {
        state.borrow().samples.iter().map(|(s, _, _)| *s).collect()
    }

    #[test]
    fn add_stores_a_sample_under_a_new_id() {
        let (mut p, state) = project();
        let a = p.add_sample(&tone()).unwrap();
        let b = p.add_sample(&tone()).unwrap();
        assert_ne!(a, b);
        assert_eq!(structure(&state), vec![a, b]);
    }

    #[test]
    fn a_stored_sample_reads_back() {
        let (mut p, _state) = project();
        let id = p.add_sample(&tone()).unwrap();
        assert_eq!(p.audio(id).unwrap().samples(), tone().samples());
    }

    #[test]
    fn reading_an_unknown_sample_is_refused() {
        let (p, _state) = project();
        let bogus = SampleId(7);
        assert!(matches!(
            p.audio(bogus),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
    }

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        audio(
            ChannelLayout::Mono,
            &(0..n).map(|i| i as Pcm * 0.01).collect::<Vec<_>>(),
        )
    }

    /// What `ramp(n)` costs the budget.
    fn ramp_bytes(n: usize) -> usize {
        n * size_of::<Pcm>()
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
        let mut p = memory_project();
        let id = p.import(&path).unwrap();
        let audio = p.audio(id).unwrap();
        assert_eq!(audio.layout(), ChannelLayout::Mono);
        assert_eq!(audio.frames(), 4);
        // Already the project rate: each sample scaled by its width, no
        // resampling.
        let scaled = |s: i16| Pcm::from(s) / 32_768.0;
        assert_eq!(
            audio.samples(),
            &[scaled(0), scaled(1000), scaled(-1000), scaled(32_767)]
        );
    }

    #[test]
    fn import_resamples_a_wav_at_another_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "b.wav", 44_100, &[0; 44_100]);
        let mut p = memory_project();
        let id = p.import(&path).unwrap();
        let frames = p.audio(id).unwrap().frames() as i64;
        assert!((frames - 48_000).abs() <= 2, "{frames}");
    }

    #[test]
    fn import_of_a_file_that_is_not_audio_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "not audio").unwrap();
        let (mut p, state) = project();
        assert!(matches!(
            p.import(&path),
            Err(ProjectError::Import(ImportError::Decode(
                DecodeError::Malformed { .. }
            )))
        ));
        assert!(structure(&state).is_empty());
    }

    #[test]
    fn import_releases_its_working_memory_and_loads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "c.wav", 44_100, &[0; 4410]);
        let mut p = memory_project();
        p.import(&path).unwrap();
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn import_that_does_not_fit_beside_loaded_audio_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav(dir.path(), "d.wav", 48_000, &[0; 100]);
        // Room for the loaded ramp and nothing else.
        let (state, storage) = memory_storage();
        let mut p = Project::on(storage, ramp_bytes(4));
        let root = p.add_sample(&ramp(4)).unwrap();
        let _held = p.load(root).unwrap();

        assert!(matches!(
            p.import(&path),
            Err(ProjectError::Import(ImportError::Decode(
                DecodeError::OverBudget { .. }
            )))
        ));
        assert_eq!(structure(&state).len(), 1, "nothing was stored");
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
    }

    #[test]
    fn load_returns_the_audio() {
        let mut p = memory_project();
        let id = p.add_sample(&ramp(4)).unwrap();

        let loaded = p.load(id).unwrap();
        assert_eq!(loaded.audio().samples(), ramp(4).samples());
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
    }

    #[test]
    fn dropping_a_loaded_sample_unloads_it() {
        let mut p = memory_project();
        let id = p.add_sample(&ramp(4)).unwrap();
        let loaded = p.load(id).unwrap();

        drop(loaded);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let mut p = memory_project();
        let id = p.add_sample(&ramp(4)).unwrap();
        let a = p.load(id).unwrap();
        let b = p.load(id).unwrap();
        assert_eq!(p.ledger().in_use(), 2 * ramp_bytes(4));
        drop(a);
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
        drop(b);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn load_over_budget_is_refused() {
        let (_state, storage) = memory_storage();
        let mut p = Project::on(storage, ramp_bytes(4));
        let a = p.add_sample(&ramp(4)).unwrap();
        let b = p.add_sample(&ramp(4)).unwrap();
        let _held = p.load(a).unwrap();

        assert!(matches!(
            p.load(b),
            Err(ProjectError::Storage(StorageError::OverBudget { .. }))
        ));
        // Refused at the allocation: nothing was brought in and let go.
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
    }

    #[test]
    fn load_of_an_unknown_sample_is_refused() {
        let mut p = memory_project();
        let root = p.add_sample(&ramp(4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            p.load(bogus),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn open_loads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::open(dir.path(), usize::MAX).unwrap();
        let id = p.add_sample(&ramp(4)).unwrap();
        let _held = p.load(id).unwrap();
        drop(p);

        let p = Project::open(dir.path(), usize::MAX).unwrap();
        assert_eq!(p.ledger().in_use(), 0, "nothing is in RAM");
        assert!(
            p.storage.has_sample(id).unwrap(),
            "the sample is still there"
        );
    }
}
