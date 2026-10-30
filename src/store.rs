//! Where a project's bytes live.
//!
//! A [`Store`] keeps one project durable: its structure and every sample's
//! audio. The engine talks to it through three operations and never learns
//! what is on the other side, so the on-disk form (a directory of WAVs
//! today, perhaps a single database file later) is the backend's private
//! business. [`crate::disk::FileStore`] is the only backend so far.
//!
//! The in-memory [`Project`] is the truth about structure and enforces the
//! lineage rules; a store is dumb and only has to keep what it is given.

use std::error::Error;

use thiserror::Error as ThisError;

use crate::audio::{AudioBuffer, AudioError};
use crate::project::{Project, ProjectError};
use crate::sample::SampleId;

/// Reasons a store cannot do what was asked.
#[derive(Debug, ThisError)]
pub enum StoreError {
    /// The stored project was written by a format this build does not read.
    #[error("stored format version {0} is not supported")]
    UnsupportedVersion(u32),
    /// The structure names a sample whose audio is not there.
    #[error("no audio stored for sample {}", .0.raw())]
    MissingAudio(SampleId),
    /// Stored audio is not in the engine's one fixed format.
    #[error("audio for sample {} is not in the engine's format: {detail}", .id.raw())]
    BadFormat { id: SampleId, detail: String },
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// The stored structure breaks a lineage rule.
    #[error(transparent)]
    Project(#[from] ProjectError),
    /// Something failed underneath: the filesystem, a codec, a database.
    #[error(transparent)]
    Backend(Box<dyn Error + Send + Sync>),
}

/// Durable storage for one project.
pub trait Store {
    /// Reads the structure of the stored project.
    ///
    /// Strict: a sample without audio, a dangling parent or an unreadable
    /// format is an error, not a dropped sample. A project that loads has
    /// its lineage intact.
    fn load(&self) -> Result<Project, StoreError>;

    /// Records a new sample's audio together with the structure that now
    /// includes it, as one durable step.
    ///
    /// `project` is the whole structure after the addition. Samples never
    /// change and ids are never reused, so a backend may keep audio it
    /// already holds for `id` rather than writing it again.
    fn put(
        &mut self,
        id: SampleId,
        audio: &AudioBuffer,
        project: &Project,
    ) -> Result<(), StoreError>;

    /// Reads one sample's audio.
    fn get(&self, id: SampleId) -> Result<AudioBuffer, StoreError>;
}

/// The behaviour every backend must have. Each backend's tests call this
/// with a way to open the same store twice, so persistence across a close
/// and reopen is checked.
#[cfg(test)]
pub(crate) fn conformance(open: impl Fn() -> Box<dyn Store>) {
    use crate::effects::reverse;

    fn ramp(channels: u16, frames: i16) -> AudioBuffer {
        let data: Vec<i16> = (0..frames * channels as i16)
            .map(|i| i * 101 - 700)
            .collect();
        AudioBuffer::new(channels, data).unwrap()
    }

    // A fresh store holds an empty project.
    let p = open().load().unwrap();
    assert_eq!(p.iter().count(), 0);
    assert_eq!(p.next_id(), 0);

    // Build a tree: root, its reverse, the reverse of that, and a branch.
    let mut store = open();
    let mut p = Project::new();
    let root = p.add(None).unwrap();
    store.put(root, &ramp(2, 16), &p).unwrap();
    let rev = p.add(Some(root)).unwrap();
    store
        .put(rev, &reverse(&store.get(root).unwrap()), &p)
        .unwrap();
    let back = p.add(Some(rev)).unwrap();
    store
        .put(back, &reverse(&store.get(rev).unwrap()), &p)
        .unwrap();
    let branch = p.add(Some(root)).unwrap();
    store.put(branch, &ramp(1, 3), &p).unwrap();
    drop(store);

    // Structure and audio survive a reopen.
    let store = open();
    let q = store.load().unwrap();
    assert_eq!(q.iter().count(), 4);
    for id in [root, rev, back, branch] {
        assert_eq!(p.get(id).unwrap().parent(), q.get(id).unwrap().parent());
    }
    assert_eq!(store.get(root).unwrap(), ramp(2, 16));
    assert_eq!(store.get(branch).unwrap(), ramp(1, 3));
    // Reverse twice is exact, through the store too.
    assert_eq!(store.get(back).unwrap(), store.get(root).unwrap());
    assert_eq!(q.next_id(), p.next_id());

    // Audio for an id the structure does not know is still an error.
    assert!(matches!(
        store.get(SampleId(99)),
        Err(StoreError::MissingAudio(SampleId(99)))
    ));
}
