//! A project kept as a directory.
//!
//! ```text
//! myproject/
//!   project.json      the manifest: format version, id counter, lineage
//!   samples/
//!     0.wav           one 48 kHz 16-bit WAV per sample, named by id
//!     1.wav
//! ```
//!
//! The manifest is a private struct here rather than a serde derive on the
//! project, so the file format is versioned on its own and the model knows
//! nothing about disk. A WAV is the buffer's bytes behind a header any audio
//! tool can open; raw bytes would need the channel count stored somewhere
//! else anyway.
//!
//! Nothing is cached: every operation reads the manifest from disk, so the
//! project is the disk. Adding a sample writes its WAV first and then
//! replaces the manifest through a temp file and rename, so a crash leaves
//! the old manifest or the new one, never half of either. The directory is
//! engine-owned; renaming a WAV by hand breaks the sample.

use std::fs;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::{MAX_CHANNELS, Pcm, SAMPLE_RATE};
use crate::project::{Sample, SampleId};
use crate::storage::{Storage, StorageError};

const MANIFEST: &str = "project.json";
const SAMPLES_DIR: &str = "samples";
const FORMAT_VERSION: u32 = 1;

/// A filesystem failure, with the path it happened at.
#[derive(Debug, Error)]
#[error("{path}: {source}")]
struct FileError {
    path: PathBuf,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

fn at(path: &Path, source: impl std::error::Error + Send + Sync + 'static) -> StorageError {
    StorageError::Backend(Box::new(FileError {
        path: path.to_path_buf(),
        source: Box::new(source),
    }))
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

impl Entry {
    fn sample(&self) -> Sample {
        Sample {
            id: SampleId(self.id),
            parent: self.parent.map(SampleId),
        }
    }
}

/// A project stored as a directory.
#[derive(Debug)]
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    /// Storage at `dir`. Nothing is touched until [`Storage::create`].
    pub fn open(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST)
    }

    fn wav_path(&self, id: SampleId) -> PathBuf {
        self.dir.join(SAMPLES_DIR).join(format!("{}.wav", id.0))
    }

    fn read_manifest(&self) -> Result<Manifest, StorageError> {
        let path = self.manifest_path();
        let bytes = fs::read(&path).map_err(|e| at(&path, e))?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| at(&path, e))?;
        if manifest.version != FORMAT_VERSION {
            return Err(StorageError::UnsupportedVersion(manifest.version));
        }
        Ok(manifest)
    }

    /// Replaces the manifest atomically: the file is either the old one or
    /// the new one.
    fn write_manifest(&self, manifest: &Manifest) -> Result<(), StorageError> {
        let path = self.manifest_path();
        let json = serde_json::to_vec_pretty(manifest).map_err(|e| at(&path, e))?;
        let tmp = self.dir.join(format!("{MANIFEST}.tmp"));
        fs::write(&tmp, json).map_err(|e| at(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| at(&path, e))
    }

    fn write_wav(&self, id: SampleId, audio: &AudioBuffer) -> Result<(), StorageError> {
        let path = self.wav_path(id);
        let spec = WavSpec {
            channels: audio.channels(),
            sample_rate: SAMPLE_RATE,
            bits_per_sample: Pcm::BITS as u16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec).map_err(|e| at(&path, e))?;
        let mut w = writer.get_i16_writer(audio.samples().len() as u32);
        for &s in audio.samples() {
            w.write_sample(s);
        }
        w.flush().map_err(|e| at(&path, e))?;
        writer.finalize().map_err(|e| at(&path, e))
    }

    fn read_wav(&self, id: SampleId) -> Result<AudioBuffer, StorageError> {
        let path = self.wav_path(id);
        let mut reader = WavReader::open(&path).map_err(|e| at(&path, e))?;
        let spec = reader.spec();
        let expected = spec.sample_rate == SAMPLE_RATE
            && spec.bits_per_sample == Pcm::BITS as u16
            && spec.sample_format == SampleFormat::Int
            && (1..=MAX_CHANNELS).contains(&spec.channels);
        if !expected {
            return Err(StorageError::BadAudioFormat {
                id,
                detail: format!("{spec:?}"),
            });
        }
        let data = reader
            .samples::<Pcm>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| at(&path, e))?;
        AudioBuffer::new(spec.channels, data).map_err(|e| at(&path, e))
    }
}

impl Storage for FileStorage {
    fn exists(&self) -> Result<bool, StorageError> {
        Ok(self.manifest_path().exists())
    }

    fn create(&mut self) -> Result<(), StorageError> {
        let samples = self.dir.join(SAMPLES_DIR);
        fs::create_dir_all(&samples).map_err(|e| at(&samples, e))?;
        self.write_manifest(&Manifest {
            version: FORMAT_VERSION,
            next_id: 0,
            samples: Vec::new(),
        })
    }

    fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError> {
        Ok(self
            .read_manifest()?
            .samples
            .iter()
            .find(|e| e.id == id.0)
            .map(Entry::sample))
    }

    fn audio(&self, id: SampleId) -> Result<Option<AudioBuffer>, StorageError> {
        if self.sample(id)?.is_none() {
            return Ok(None);
        }
        self.read_wav(id).map(Some)
    }

    fn has_children(&self, id: SampleId) -> Result<bool, StorageError> {
        Ok(self
            .read_manifest()?
            .samples
            .iter()
            .any(|e| e.parent == Some(id.0)))
    }

    fn remove_sample(&mut self, id: SampleId) -> Result<(), StorageError> {
        let mut manifest = self.read_manifest()?;
        let before = manifest.samples.len();
        manifest.samples.retain(|e| e.id != id.0);
        if manifest.samples.len() == before {
            return Ok(());
        }
        self.write_manifest(&manifest)?;
        let path = self.wav_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(at(&path, e)),
        }
    }

    fn add_sample(
        &mut self,
        parent: Option<SampleId>,
        audio: &AudioBuffer,
    ) -> Result<SampleId, StorageError> {
        let mut manifest = self.read_manifest()?;
        let id = SampleId(manifest.next_id);
        manifest.next_id += 1;
        self.write_wav(id, audio)?;
        manifest.samples.push(Entry {
            id: id.0,
            parent: parent.map(|p| p.0),
        });
        self.write_manifest(&manifest)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::reverse;

    fn ramp(channels: u16, frames: i16) -> AudioBuffer {
        let data: Vec<i16> = (0..frames * channels as i16)
            .map(|i| i * 101 - 700)
            .collect();
        AudioBuffer::new(channels, data).unwrap()
    }

    fn created(dir: &Path) -> FileStorage {
        let mut s = FileStorage::open(dir);
        s.create().unwrap();
        s
    }

    #[test]
    fn nothing_exists_until_created() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = FileStorage::open(dir.path());
        assert!(!s.exists().unwrap());
        s.create().unwrap();
        assert!(s.exists().unwrap());
        assert!(dir.path().join(MANIFEST).exists());
        assert!(dir.path().join(SAMPLES_DIR).is_dir());
    }

    #[test]
    fn samples_and_audio_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let root = s.add_sample(None, &ramp(2, 16)).unwrap();
        let rev = s
            .add_sample(Some(root), &reverse(&s.audio(root).unwrap().unwrap()))
            .unwrap();
        let back = s
            .add_sample(Some(rev), &reverse(&s.audio(rev).unwrap().unwrap()))
            .unwrap();
        drop(s);

        let s = FileStorage::open(dir.path());
        assert!(s.exists().unwrap());
        assert_eq!(s.sample(rev).unwrap().unwrap().parent, Some(root));
        assert_eq!(s.sample(back).unwrap().unwrap().parent, Some(rev));
        assert_eq!(s.audio(root).unwrap().unwrap(), ramp(2, 16));
        // Reverse twice is exact, through the disk too.
        assert_eq!(s.audio(back).unwrap(), s.audio(root).unwrap());
    }

    #[test]
    fn unknown_id_is_none_for_sample_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let s = created(dir.path());
        assert!(s.sample(SampleId(9)).unwrap().is_none());
        assert!(s.audio(SampleId(9)).unwrap().is_none());
    }

    #[test]
    fn ids_are_not_reused_across_remove_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let a = s.add_sample(None, &ramp(1, 1)).unwrap();
        s.remove_sample(a).unwrap();
        drop(s);
        let mut s = FileStorage::open(dir.path());
        let b = s.add_sample(None, &ramp(1, 1)).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn remove_deletes_the_wav_and_has_children_sees_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let root = s.add_sample(None, &ramp(1, 1)).unwrap();
        let child = s.add_sample(Some(root), &ramp(1, 1)).unwrap();
        assert!(s.has_children(root).unwrap());
        assert!(!s.has_children(child).unwrap());

        s.remove_sample(child).unwrap();
        assert!(!s.has_children(root).unwrap());
        assert!(!s.wav_path(child).exists());
        assert!(s.wav_path(root).exists());
        // Removing again is not an error.
        s.remove_sample(child).unwrap();
    }

    #[test]
    fn renamed_wav_is_an_error_for_audio() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(1, 4)).unwrap();
        fs::rename(s.wav_path(id), dir.path().join("samples/kick.wav")).unwrap();
        assert!(s.sample(id).unwrap().is_some());
        assert!(matches!(s.audio(id), Err(StorageError::Backend(_))));
    }

    #[test]
    fn wrong_sample_rate_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(1, 4)).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(s.wav_path(id), spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        assert!(matches!(
            s.audio(id),
            Err(StorageError::BadAudioFormat { id: i, .. }) if i == id
        ));
    }

    #[test]
    fn unsupported_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let s = created(dir.path());
        let manifest = r#"{"version":99,"next_id":0,"samples":[]}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        assert!(matches!(
            s.sample(SampleId(0)),
            Err(StorageError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(None, &ramp(1, 1)).unwrap();
        assert!(!dir.path().join("project.json.tmp").exists());
    }
}
