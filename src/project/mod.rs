//! A project: everything the device keeps for one piece of work.
//!
//! [`Project`] is the rules over its storage and keeps no copy: every
//! change is on disk when the call returns, so there is no save step. It
//! holds a `SampleMemory`, so every byte of audio it brings into RAM is paid
//! for. Project and engine never name each other; the caller carries a
//! [`Sample`] between them.

pub(crate) mod storage;

use std::path::Path;

use thiserror::Error;

use crate::audio::{AudioBuffer, ChannelLayout, Sample};
use crate::memory::{OverBudgetError, SampleMemory};
use storage::{Storage, StorageError};

/// Minted by storage when a sample is added, never reused, and
/// meaningless in another project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Everything stored about a sample but its audio and id. Crate-private
/// because it is part of the storage format; empty until prints bring
/// back the parent pointer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SampleMetadata {}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("no sample with id {}", .0.0)]
    UnknownSample(SampleId),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    OverBudget(#[from] OverBudgetError),
}

pub struct Project {
    storage: Storage,
    memory: SampleMemory,
}

impl Project {
    /// The project at `dir`, created if the directory is empty or missing,
    /// validated otherwise. `budget` is the most bytes of audio held in RAM
    /// at once: the caller knows the machine. Nothing is loaded.
    pub fn open(dir: &Path, budget: usize) -> Result<Self, ProjectError> {
        Ok(Self {
            storage: Storage::open(dir)?,
            memory: SampleMemory::with_budget(budget),
        })
    }

    /// Reads `id` into RAM; dropping the sample unloads it once nothing
    /// playing it still holds it. Each load is its own copy, so loading
    /// twice costs the memory twice.
    pub fn load(&self, id: SampleId) -> Result<Sample, ProjectError> {
        let audio = self
            .storage
            .audio(id, &self.memory)?
            .ok_or(ProjectError::UnknownSample(id))?;
        Ok(Sample::new(audio))
    }

    /// An empty sample with room for `frames`, paid for in full now. It
    /// has no id until it is added.
    pub fn alloc(&self, layout: ChannelLayout, frames: usize) -> Result<Sample, ProjectError> {
        // Saturating: memory refuses usize::MAX samples, whose bytes overflow.
        let samples = frames.saturating_mul(layout.count());
        let block = self.memory.alloc(samples)?;
        let audio = AudioBuffer::new(layout, block)
            .unwrap_or_else(|_| unreachable!("an empty block is whole frames"));
        Ok(Sample::new(audio))
    }

    /// Stores a copy of `sample` on disk and mints its id.
    pub fn add_sample(&mut self, sample: &Sample) -> Result<SampleId, ProjectError> {
        Ok(self
            .storage
            .add_sample(&SampleMetadata {}, sample.audio())?)
    }

    #[cfg(test)]
    pub(crate) fn memory(&self) -> &SampleMemory {
        &self.memory
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::mem::size_of;

    use tempfile::TempDir;

    use super::*;
    use crate::config::Pcm;
    use crate::testing::audio;

    /// Dropping the directory deletes the project.
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
        assert_eq!(p.memory().in_use(), ramp_bytes(4));
    }

    #[test]
    fn dropping_a_loaded_sample_unloads_it() {
        let (_dir, mut p) = project(usize::MAX);
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let loaded = p.load(id).unwrap();

        drop(loaded);
        assert_eq!(p.memory().in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let (_dir, mut p) = project(usize::MAX);
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let a = p.load(id).unwrap();
        let b = p.load(id).unwrap();
        assert_eq!(p.memory().in_use(), 2 * ramp_bytes(4));
        drop(a);
        assert_eq!(p.memory().in_use(), ramp_bytes(4));
        drop(b);
        assert_eq!(p.memory().in_use(), 0);
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
        assert_eq!(p.memory().in_use(), ramp_bytes(4));
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
        assert_eq!(p.memory().in_use(), 0);
    }

    #[test]
    fn alloc_pays_for_the_room_and_holds_nothing() {
        let (_dir, p) = project(usize::MAX);
        let sample = p.alloc(ChannelLayout::Stereo, 4).unwrap();
        assert_eq!(sample.audio().frames(), 0);
        assert_eq!(p.memory().in_use(), 2 * ramp_bytes(4));
        drop(sample);
        assert_eq!(p.memory().in_use(), 0);
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
        assert_eq!(p.memory().in_use(), 0);
    }

    #[test]
    fn open_loads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::open(dir.path(), usize::MAX).unwrap();
        let id = p.add_sample(&Sample::new(ramp(4))).unwrap();
        let _held = p.load(id).unwrap();
        drop(p);

        let p = Project::open(dir.path(), usize::MAX).unwrap();
        assert_eq!(p.memory().in_use(), 0, "nothing is in RAM");
        assert!(
            p.storage.has_sample(id).unwrap(),
            "the sample is still there"
        );
    }
}
