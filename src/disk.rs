//! A project as a self-contained directory: the file-based [`Store`].
//!
//! ```text
//! myproject/
//!   project.json      the manifest: format version, id counter, lineage
//!   samples/
//!     0.wav           one 48 kHz 16-bit WAV per sample, named by id
//!     1.wav
//! ```
//!
//! The manifest is a private struct here rather than a serde derive on
//! [`Project`], so the file format is versioned on its own and the model
//! knows nothing about disk. A WAV is the buffer's bytes behind a header
//! any audio tool can open; raw bytes would need the channel count stored
//! somewhere else anyway.
//!
//! A put writes the new WAV, then replaces the manifest through a temp file
//! and rename. A crash leaves the old or the new manifest, and a WAV the
//! manifest does not mention is ignored. The directory is engine-owned:
//! renaming a WAV by hand breaks the project.

use std::fs;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::{MAX_CHANNELS, Pcm, SAMPLE_RATE};
use crate::project::Project;
use crate::sample::SampleId;
use crate::store::{Store, StoreError};

const MANIFEST: &str = "project.json";
const SAMPLES_DIR: &str = "samples";
const FORMAT_VERSION: u32 = 1;

/// A filesystem failure, with the path it happened at.
#[derive(Debug, Error)]
#[error("{path}: {source}")]
pub struct FileError {
    path: PathBuf,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl FileError {
    /// Wraps a failure at `path` as a backend error for the store.
    fn at(path: &Path, source: impl std::error::Error + Send + Sync + 'static) -> StoreError {
        StoreError::Backend(Box::new(Self {
            path: path.to_path_buf(),
            source: Box::new(source),
        }))
    }
}

/// The on-disk description of a project. Everything but the audio.
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    next_id: u32,
    samples: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    id: u32,
    parent: Option<u32>,
}

/// A project stored as a directory.
#[derive(Debug)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Opens the project directory at `dir`, creating an empty project
    /// there if it does not hold one yet.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let store = Self {
            dir: dir.to_path_buf(),
        };
        let samples = store.samples_dir();
        fs::create_dir_all(&samples).map_err(|e| FileError::at(&samples, e))?;
        if !store.manifest_path().exists() {
            store.write_manifest(&Project::new())?;
        }
        Ok(store)
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST)
    }

    fn samples_dir(&self) -> PathBuf {
        self.dir.join(SAMPLES_DIR)
    }

    fn wav_path(&self, id: SampleId) -> PathBuf {
        self.samples_dir().join(format!("{}.wav", id.raw()))
    }

    fn write_manifest(&self, project: &Project) -> Result<(), StoreError> {
        let mut samples: Vec<Entry> = project
            .iter()
            .map(|s| Entry {
                id: s.id().raw(),
                parent: s.parent().map(SampleId::raw),
            })
            .collect();
        samples.sort_by_key(|e| e.id);
        let manifest = Manifest {
            version: FORMAT_VERSION,
            next_id: project.next_id(),
            samples,
        };
        let path = self.manifest_path();
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| FileError::at(&path, e))?;
        let tmp = self.dir.join(format!("{MANIFEST}.tmp"));
        fs::write(&tmp, json).map_err(|e| FileError::at(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| FileError::at(&path, e))
    }

    fn write_wav(&self, id: SampleId, audio: &AudioBuffer) -> Result<(), StoreError> {
        let path = self.wav_path(id);
        let spec = WavSpec {
            channels: audio.channels(),
            sample_rate: SAMPLE_RATE,
            bits_per_sample: Pcm::BITS as u16,
            sample_format: SampleFormat::Int,
        };
        let err = |e| FileError::at(&path, e);
        let mut writer = WavWriter::create(&path, spec).map_err(err)?;
        let mut w = writer.get_i16_writer(audio.samples().len() as u32);
        for &s in audio.samples() {
            w.write_sample(s);
        }
        w.flush().map_err(err)?;
        writer.finalize().map_err(err)
    }
}

impl Store for FileStore {
    fn load(&self) -> Result<Project, StoreError> {
        let path = self.manifest_path();
        let bytes = fs::read(&path).map_err(|e| FileError::at(&path, e))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| FileError::at(&path, e))?;
        if manifest.version != FORMAT_VERSION {
            return Err(StoreError::UnsupportedVersion(manifest.version));
        }
        for e in &manifest.samples {
            let id = SampleId(e.id);
            if !self.wav_path(id).exists() {
                return Err(StoreError::MissingAudio(id));
            }
        }
        let samples = manifest
            .samples
            .iter()
            .map(|e| (SampleId(e.id), e.parent.map(SampleId)));
        Ok(Project::restore(samples, manifest.next_id)?)
    }

    fn put(
        &mut self,
        id: SampleId,
        audio: &AudioBuffer,
        project: &Project,
    ) -> Result<(), StoreError> {
        if !self.wav_path(id).exists() {
            self.write_wav(id, audio)?;
        }
        self.write_manifest(project)
    }

    fn get(&self, id: SampleId) -> Result<AudioBuffer, StoreError> {
        let path = self.wav_path(id);
        if !path.exists() {
            return Err(StoreError::MissingAudio(id));
        }
        let err = |e| FileError::at(&path, e);
        let mut reader = WavReader::open(&path).map_err(err)?;
        let spec = reader.spec();
        let expected = spec.sample_rate == SAMPLE_RATE
            && spec.bits_per_sample == Pcm::BITS as u16
            && spec.sample_format == SampleFormat::Int
            && (1..=MAX_CHANNELS).contains(&spec.channels);
        if !expected {
            return Err(StoreError::BadFormat {
                id,
                detail: format!("{spec:?}"),
            });
        }
        let data = reader
            .samples::<Pcm>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        Ok(AudioBuffer::new(spec.channels, data)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::conformance;

    fn one(channels: u16) -> AudioBuffer {
        AudioBuffer::new(channels, vec![0; channels as usize * 4]).unwrap()
    }

    #[test]
    fn file_store_conforms() {
        let dir = tempfile::tempdir().unwrap();
        conformance(|| Box::new(FileStore::open(dir.path()).unwrap()));
    }

    #[test]
    fn open_writes_an_empty_manifest_at_once() {
        let dir = tempfile::tempdir().unwrap();
        FileStore::open(dir.path()).unwrap();
        assert!(dir.path().join(MANIFEST).exists());
        assert!(dir.path().join(SAMPLES_DIR).is_dir());
    }

    #[test]
    fn renamed_wav_refuses_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::open(dir.path()).unwrap();
        let mut p = Project::new();
        let id = p.add(None).unwrap();
        store.put(id, &one(1), &p).unwrap();
        fs::rename(store.wav_path(id), dir.path().join("samples/kick.wav")).unwrap();
        assert!(matches!(store.load(), Err(StoreError::MissingAudio(i)) if i == id));
    }

    #[test]
    fn dangling_parent_refuses_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::open(dir.path()).unwrap();
        let mut p = Project::new();
        let root = p.add(None).unwrap();
        store.put(root, &one(1), &p).unwrap();
        let child = p.add(Some(root)).unwrap();
        store.put(child, &one(1), &p).unwrap();
        // The manifest forgets the root but the child still points at it.
        let manifest = r#"{"version":1,"next_id":2,"samples":[{"id":1,"parent":0}]}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        assert!(matches!(
            store.load(),
            Err(StoreError::Project(
                crate::project::ProjectError::UnknownSample(SampleId(0))
            ))
        ));
    }

    #[test]
    fn unsupported_version_refuses_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path()).unwrap();
        let manifest = r#"{"version":99,"next_id":0,"samples":[]}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        assert!(matches!(
            store.load(),
            Err(StoreError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn wrong_sample_rate_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::open(dir.path()).unwrap();
        let mut p = Project::new();
        let id = p.add(None).unwrap();
        store.put(id, &one(1), &p).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(store.wav_path(id), spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        assert!(matches!(store.get(id), Err(StoreError::BadFormat { .. })));
    }

    #[test]
    fn put_does_not_rewrite_audio_it_already_holds() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = FileStore::open(dir.path()).unwrap();
        let mut p = Project::new();
        let id = p.add(None).unwrap();
        store.put(id, &one(1), &p).unwrap();
        let before = fs::metadata(store.wav_path(id))
            .unwrap()
            .modified()
            .unwrap();
        store.put(id, &one(2), &p).unwrap();
        let after = fs::metadata(store.wav_path(id))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(store.get(id).unwrap(), one(1));
    }
}
