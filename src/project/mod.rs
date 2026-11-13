//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over a `Storage`, and the one way audio
//! moves between the disk and RAM. It keeps no copy of the data: every
//! read and write goes to storage, so changing the project is changing
//! the disk and there is no save step. Today a project holds samples;
//! patterns and settings arrive as their steps come.
//!
//! The project makes and holds the `Ledger`, because every byte of
//! audio it brings into RAM, a sample allocated to be written or one
//! loaded for playback, is a block on it. What it hands out is a
//! [`Sample`], plain audio owned by whoever holds it; the project
//! knows nothing about sound, and the renderer that plays the sample
//! knows nothing about the project. The caller carries the sample from
//! one to the other.

pub(crate) mod ledger;
pub(crate) mod storage;

use std::path::Path;

use thiserror::Error;

use crate::audio::{AudioBuffer, ChannelLayout, Sample};
use crate::project::ledger::{Ledger, OverBudgetError};
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
    OverBudget(#[from] OverBudgetError),
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

    /// An empty sample with room for `frames` frames of `layout`, paid
    /// for against the budget now, in full.
    ///
    /// Nothing is stored: the sample has no id until it is added.
    pub fn alloc(&self, layout: ChannelLayout, frames: usize) -> Result<Sample, ProjectError> {
        // Saturating: a count past usize cannot be afforded, and the
        // ledger refuses it as over budget.
        let samples = frames.saturating_mul(layout.count());
        let block = self.ledger.alloc(samples)?;
        let audio = AudioBuffer::new(layout, block)
            .unwrap_or_else(|_| unreachable!("an empty block is whole frames"));
        Ok(Sample::new(audio))
    }

    /// Stores `sample` as a new sample and returns its id. The id is
    /// minted here: until it is added, a sample has none.
    ///
    /// The project keeps its own copy on disk; the sample stays the
    /// caller's, unchanged.
    pub fn add_sample(&mut self, sample: &Sample) -> Result<SampleId, ProjectError> {
        Ok(self
            .storage
            .add_sample(&SampleMetadata {}, sample.audio())?)
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

    use super::*;
    use crate::audio::ChannelLayout;
    use crate::config::Pcm;
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
        let a = p.add_sample(&Sample::new(tone())).unwrap();
        let b = p.add_sample(&Sample::new(tone())).unwrap();
        assert_ne!(a, b);
        assert_eq!(structure(&state), vec![a, b]);
    }

    #[test]
    fn a_stored_sample_reads_back() {
        let (mut p, _state) = project();
        let id = p.add_sample(&Sample::new(tone())).unwrap();
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

    #[test]
    fn load_returns_the_audio() {
        let mut p = memory_project();
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();

        let loaded = p.load(id).unwrap();
        assert_eq!(loaded.audio().samples(), ramp(4).samples());
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
    }

    #[test]
    fn dropping_a_loaded_sample_unloads_it() {
        let mut p = memory_project();
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let loaded = p.load(id).unwrap();

        drop(loaded);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let mut p = memory_project();
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
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
        let a = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let b = p.add_sample(&Sample::new(ramp(4))).unwrap();
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
        let root = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            p.load(bogus),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn alloc_pays_for_the_room_and_holds_nothing() {
        let p = memory_project();
        let sample = p.alloc(ChannelLayout::Stereo, 4).unwrap();
        assert_eq!(sample.audio().frames(), 0);
        assert_eq!(p.ledger().in_use(), 2 * ramp_bytes(4));
        drop(sample);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn alloc_over_budget_is_refused() {
        let (_state, storage) = memory_storage();
        let p = Project::on(storage, ramp_bytes(4));
        assert!(matches!(
            p.alloc(ChannelLayout::Mono, 5),
            Err(ProjectError::OverBudget(_))
        ));
        assert!(matches!(
            p.alloc(ChannelLayout::Stereo, usize::MAX),
            Err(ProjectError::OverBudget(_))
        ));
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn open_loads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::open(dir.path(), usize::MAX).unwrap();
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
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
