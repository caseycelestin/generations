//! A project: everything the device keeps for one piece of work.
//!
//! [`Project`] is the rules over its storage and keeps no copy: every
//! change is on disk when the call returns, so there is no save step. It
//! holds no memory: a load is paid for from the caller's `AudioMemory`.
//! Project and engine never name each other; the caller carries a
//! [`Audio`] between them.

pub(crate) mod storage;

use std::path::Path;

use thiserror::Error;

use crate::audio::{Audio, AudioMemory};
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
}

pub struct Project {
    storage: Storage,
}

impl Project {
    /// The project at `dir`, created if the directory is empty or missing,
    /// validated otherwise. Nothing is loaded.
    pub fn open(dir: &Path) -> Result<Self, ProjectError> {
        Ok(Self {
            storage: Storage::open(dir)?,
        })
    }

    /// Reads `id` into RAM; dropping the audio unloads it once nothing
    /// playing it still holds it. Each load is its own copy, so loading
    /// twice costs the memory twice.
    pub fn load(&self, id: SampleId, memory: &AudioMemory) -> Result<Audio, ProjectError> {
        let buffer = self
            .storage
            .audio(id, memory)?
            .ok_or(ProjectError::UnknownSample(id))?;
        Ok(Audio::new(buffer))
    }

    /// Stores a copy of `audio` on disk as a new sample and mints its id.
    pub fn add_sample(&mut self, audio: &Audio) -> Result<SampleId, ProjectError> {
        Ok(self
            .storage
            .add_sample(&SampleMetadata {}, audio.buffer())?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::mem::size_of;

    use tempfile::TempDir;

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::Pcm;
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    /// Dropping the directory deletes the project.
    fn project() -> (TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::open(dir.path()).unwrap();
        (dir, p)
    }

    fn tone() -> AudioBuffer {
        buffer(ChannelLayout::Mono, &[0.0, 0.1, 0.2, 0.3])
    }

    #[test]
    fn add_stores_a_sample_under_a_new_id() {
        let (_dir, mut p) = project();
        let a = p.add_sample(&Audio::new(tone())).unwrap();
        let b = p.add_sample(&Audio::new(tone())).unwrap();
        assert_ne!(a, b);
        assert!(p.storage.has_sample(a).unwrap());
        assert!(p.storage.has_sample(b).unwrap());
    }

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        buffer(
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
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&Audio::new(ramp(4))).unwrap();

        let loaded = p.load(id, &memory).unwrap();
        assert_eq!(loaded.samples(), ramp(4).samples());
        assert_eq!(memory.in_use(), ramp_bytes(4));
    }

    #[test]
    fn dropping_a_loaded_sample_unloads_it() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&Audio::new(ramp(4))).unwrap();
        let loaded = p.load(id, &memory).unwrap();

        drop(loaded);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&Audio::new(ramp(4))).unwrap();
        let a = p.load(id, &memory).unwrap();
        let b = p.load(id, &memory).unwrap();
        assert_eq!(memory.in_use(), 2 * ramp_bytes(4));
        drop(a);
        assert_eq!(memory.in_use(), ramp_bytes(4));
        drop(b);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn load_over_budget_is_refused() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::with_budget(ramp_bytes(4));
        let a = p.add_sample(&Audio::new(ramp(4))).unwrap();
        let b = p.add_sample(&Audio::new(ramp(4))).unwrap();
        let _held = p.load(a, &memory).unwrap();

        assert!(matches!(
            p.load(b, &memory),
            Err(ProjectError::Storage(StorageError::OverBudget { .. }))
        ));
        // Refused at the allocation: nothing was brought in and let go.
        assert_eq!(memory.in_use(), ramp_bytes(4));
    }

    #[test]
    fn load_of_an_unknown_sample_is_refused() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let root = p.add_sample(&Audio::new(ramp(4))).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            p.load(bogus, &memory),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert_eq!(memory.in_use(), 0);
    }
}
