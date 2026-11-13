//! A project: everything the device keeps for one piece of work.
//!
//! A [`Project`] is the rules over its storage, and the one way audio
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
    storage: Storage,
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
    /// An empty project is created if the directory is empty or missing;
    /// the whole project found is validated otherwise, and a directory
    /// holding anything else is refused. From then on every change is
    /// durable by the time the call that made it returns.
    pub fn open(dir: &Path, budget: usize) -> Result<Self, ProjectError> {
        Ok(Self {
            storage: Storage::open(dir)?,
            ledger: Ledger::new(budget),
        })
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
    use std::mem::size_of;

    use tempfile::TempDir;

    use super::*;
    use crate::config::Pcm;
    use crate::testing::audio;

    /// A project in a fresh directory, with the directory to keep it alive.
    fn project(budget: usize) -> (TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::open(dir.path(), budget).unwrap();
        (dir, p)
    }

    fn tone() -> AudioBuffer {
        audio(ChannelLayout::Mono, &[0.0, 0.1, 0.2, 0.3])
    }

    #[test]
    fn add_stores_a_sample_under_a_new_id() {
        let (_dir, mut p) = project(usize::MAX);
        let a = p.add_sample(&Sample::new(tone())).unwrap();
        let b = p.add_sample(&Sample::new(tone())).unwrap();
        assert_ne!(a, b);
        assert!(p.storage.has_sample(a).unwrap());
        assert!(p.storage.has_sample(b).unwrap());
    }

    #[test]
    fn a_stored_sample_reads_back() {
        let (_dir, mut p) = project(usize::MAX);
        let id = p.add_sample(&Sample::new(tone())).unwrap();
        assert_eq!(p.audio(id).unwrap().samples(), tone().samples());
    }

    #[test]
    fn reading_an_unknown_sample_is_refused() {
        let (_dir, p) = project(usize::MAX);
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
        let (_dir, mut p) = project(usize::MAX);
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();

        let loaded = p.load(id).unwrap();
        assert_eq!(loaded.audio().samples(), ramp(4).samples());
        assert_eq!(p.ledger().in_use(), ramp_bytes(4));
    }

    #[test]
    fn dropping_a_loaded_sample_unloads_it() {
        let (_dir, mut p) = project(usize::MAX);
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let loaded = p.load(id).unwrap();

        drop(loaded);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let (_dir, mut p) = project(usize::MAX);
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
        let (_dir, mut p) = project(ramp_bytes(4));
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
        let (_dir, mut p) = project(usize::MAX);
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
        let (_dir, p) = project(usize::MAX);
        let sample = p.alloc(ChannelLayout::Stereo, 4).unwrap();
        assert_eq!(sample.audio().frames(), 0);
        assert_eq!(p.ledger().in_use(), 2 * ramp_bytes(4));
        drop(sample);
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn alloc_over_budget_is_refused() {
        let (_dir, p) = project(ramp_bytes(4));
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
