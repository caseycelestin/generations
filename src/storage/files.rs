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
//! project is the disk. Adding a sample writes and syncs its WAV first,
//! then replaces the manifest through a synced temp file and rename, with
//! the directory synced after each step, so a power cut leaves the old
//! manifest or the new one, never half of either. The directory is
//! engine-owned; renaming a WAV by hand breaks the sample, and
//! [`FileStorage::open`] checks the whole project before opening on it.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::config::{MAX_CHANNELS, Pcm, SAMPLE_RATE};
use crate::ledger::Reservation;
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
///
/// Samples are a map keyed by id, so an id cannot appear twice and the
/// file is written in id order.
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    next_id: u32,
    samples: BTreeMap<u32, Entry>,
}

/// Everything stored about one sample except its audio and its id, which
/// is the map key.
#[derive(Serialize, Deserialize)]
struct Entry {
    parent: Option<u32>,
    /// Absent in manifests written before the field existed, which means
    /// not loaded, so the format version does not move.
    #[serde(default)]
    loaded: bool,
}

/// A project stored as a directory.
#[derive(Debug)]
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    /// Storage at `dir`.
    ///
    /// If the directory is empty or missing, an empty project is created
    /// there. If it holds a project, the whole project is checked before
    /// this returns. Anything else is an error: a broken project, or files
    /// that are not a project at all.
    pub fn open(dir: &Path) -> Result<Self, StorageError> {
        let storage = Self {
            dir: dir.to_path_buf(),
        };
        if storage.manifest_path().exists() {
            storage.validate()?;
        } else if storage.dir_is_empty_or_missing()? {
            storage.create()?;
        } else {
            return Err(StorageError::NotAProject(format!(
                "{} has files but no {MANIFEST}",
                storage.dir.display()
            )));
        }
        Ok(storage)
    }

    fn dir_is_empty_or_missing(&self) -> Result<bool, StorageError> {
        match fs::read_dir(&self.dir) {
            Ok(mut entries) => Ok(entries.next().is_none()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(at(&self.dir, e)),
        }
    }

    fn create(&self) -> Result<(), StorageError> {
        let samples = self.samples_dir();
        fs::create_dir_all(&samples).map_err(|e| at(&samples, e))?;
        self.write_manifest(&Manifest {
            version: FORMAT_VERSION,
            next_id: 0,
            samples: BTreeMap::new(),
        })
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST)
    }

    fn samples_dir(&self) -> PathBuf {
        self.dir.join(SAMPLES_DIR)
    }

    fn wav_path(&self, id: SampleId) -> PathBuf {
        self.samples_dir().join(format!("{}.wav", id.0))
    }

    /// Checks that what is stored is a whole project: the manifest reads,
    /// every parent exists and was minted before its child, the counter is
    /// past every id, and every sample's WAV is present with a header in
    /// the engine's format.
    fn validate(&self) -> Result<(), StorageError> {
        let manifest = self.read_manifest()?;
        let corrupt = |detail: String| StorageError::Corrupt(detail);
        for (&id, entry) in &manifest.samples {
            if let Some(parent) = entry.parent {
                if parent >= id {
                    return Err(corrupt(format!(
                        "sample {id} has parent {parent}, which does not come before it"
                    )));
                }
                if !manifest.samples.contains_key(&parent) {
                    return Err(corrupt(format!(
                        "sample {id} has parent {parent}, which does not exist"
                    )));
                }
            }
            self.open_wav(SampleId(id))?;
        }
        if let Some(&max) = manifest.samples.keys().next_back()
            && manifest.next_id <= max
        {
            return Err(corrupt(format!(
                "next id {} does not follow the largest sample id {max}",
                manifest.next_id
            )));
        }
        Ok(())
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

    /// Replaces the manifest atomically and durably: after this returns
    /// the file on disk is either the old one or the new one, even across
    /// a power cut.
    fn write_manifest(&self, manifest: &Manifest) -> Result<(), StorageError> {
        let path = self.manifest_path();
        let json = serde_json::to_vec_pretty(manifest).map_err(|e| at(&path, e))?;
        let tmp = self.dir.join(format!("{MANIFEST}.tmp"));
        write_synced(&tmp, &json)?;
        fs::rename(&tmp, &path).map_err(|e| at(&path, e))?;
        sync_dir(&self.dir)
    }

    /// Writes a sample's WAV and syncs it, so that by the time the manifest
    /// names the sample its audio is on disk.
    fn write_wav(&self, id: SampleId, audio: &AudioBuffer) -> Result<(), StorageError> {
        let path = self.wav_path(id);
        let spec = WavSpec {
            channels: audio.channels(),
            sample_rate: SAMPLE_RATE,
            bits_per_sample: Pcm::BITS as u16,
            sample_format: SampleFormat::Int,
        };
        let mut file = File::create(&path).map_err(|e| at(&path, e))?;
        {
            let mut writer = WavWriter::new(&mut file, spec).map_err(|e| at(&path, e))?;
            let mut w = writer.get_i16_writer(audio.samples().len() as u32);
            for &s in audio.samples() {
                w.write_sample(s);
            }
            w.flush().map_err(|e| at(&path, e))?;
            writer.finalize().map_err(|e| at(&path, e))?;
        }
        file.sync_all().map_err(|e| at(&path, e))?;
        sync_dir(&self.samples_dir())
    }

    /// Opens a sample's WAV and checks its header, reading no audio.
    fn open_wav(&self, id: SampleId) -> Result<WavReader<std::io::BufReader<File>>, StorageError> {
        let path = self.wav_path(id);
        let reader = WavReader::open(&path).map_err(|e| at(&path, e))?;
        check_spec(id, reader.spec())?;
        Ok(reader)
    }

    fn read_wav(
        &self,
        id: SampleId,
        reservation: &Reservation,
    ) -> Result<AudioBuffer, StorageError> {
        let path = self.wav_path(id);
        let mut reader = self.open_wav(id)?;
        let spec = reader.spec();
        reservation
            .claim(AudioBuffer::bytes_for(
                spec.channels,
                reader.duration() as usize,
            ))
            .map_err(|source| StorageError::ShortReservation { id, source })?;
        // Exactly the capacity reserved: collecting through a `Result`
        // loses the size hint and grows by doubling.
        let mut data = Vec::with_capacity(reader.len() as usize);
        for sample in reader.samples::<Pcm>() {
            data.push(sample.map_err(|e| at(&path, e))?);
        }
        AudioBuffer::new(spec.channels, data).map_err(|e| at(&path, e))
    }
}

/// Refuses a WAV header that is not the engine's one fixed format.
fn check_spec(id: SampleId, spec: WavSpec) -> Result<(), StorageError> {
    let expected = spec.sample_rate == SAMPLE_RATE
        && spec.bits_per_sample == Pcm::BITS as u16
        && spec.sample_format == SampleFormat::Int
        && (1..=MAX_CHANNELS).contains(&spec.channels);
    if expected {
        Ok(())
    } else {
        Err(StorageError::BadAudioFormat {
            id,
            detail: format!("{spec:?}"),
        })
    }
}

/// Writes a whole file and syncs it to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    use std::io::Write;
    let mut file = File::create(path).map_err(|e| at(path, e))?;
    file.write_all(bytes).map_err(|e| at(path, e))?;
    file.sync_all().map_err(|e| at(path, e))
}

/// Syncs a directory so that a file created, renamed or removed in it is
/// durably in it.
fn sync_dir(dir: &Path) -> Result<(), StorageError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| at(dir, e))
}

impl Storage for FileStorage {
    fn has_sample(&self, id: SampleId) -> Result<bool, StorageError> {
        Ok(self.read_manifest()?.samples.contains_key(&id.0))
    }

    fn sample(&self, id: SampleId) -> Result<Option<Sample>, StorageError> {
        let manifest = self.read_manifest()?;
        let Some(entry) = manifest.samples.get(&id.0) else {
            return Ok(None);
        };
        // The WAV header is the one record of the audio's shape; the
        // manifest does not repeat it, so the two cannot disagree.
        let reader = self.open_wav(id)?;
        Ok(Some(Sample {
            id,
            parent: entry.parent.map(SampleId),
            loaded: entry.loaded,
            channels: reader.spec().channels,
            frames: reader.duration() as usize,
        }))
    }

    fn audio(
        &self,
        id: SampleId,
        reservation: &Reservation,
    ) -> Result<Option<AudioBuffer>, StorageError> {
        if !self.has_sample(id)? {
            return Ok(None);
        }
        self.read_wav(id, reservation).map(Some)
    }

    fn has_children(&self, id: SampleId) -> Result<bool, StorageError> {
        Ok(self
            .read_manifest()?
            .samples
            .values()
            .any(|e| e.parent == Some(id.0)))
    }

    fn loaded_samples(&self) -> Result<Vec<SampleId>, StorageError> {
        Ok(self
            .read_manifest()?
            .samples
            .iter()
            .filter(|(_, e)| e.loaded)
            .map(|(&id, _)| SampleId(id))
            .collect())
    }

    fn remove_sample(&mut self, id: SampleId) -> Result<(), StorageError> {
        let mut manifest = self.read_manifest()?;
        if manifest.samples.remove(&id.0).is_none() {
            return Ok(());
        }
        self.write_manifest(&manifest)?;
        let path = self.wav_path(id);
        match fs::remove_file(&path) {
            Ok(()) => sync_dir(&self.samples_dir()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(at(&path, e)),
        }
    }

    fn set_loaded(&mut self, id: SampleId, loaded: bool) -> Result<(), StorageError> {
        let mut manifest = self.read_manifest()?;
        let Some(entry) = manifest.samples.get_mut(&id.0) else {
            return Ok(());
        };
        if entry.loaded == loaded {
            return Ok(());
        }
        entry.loaded = loaded;
        self.write_manifest(&manifest)
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
        manifest.samples.insert(
            id.0,
            Entry {
                parent: parent.map(|p| p.0),
                loaded: false,
            },
        );
        self.write_manifest(&manifest)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Ledger, ShortReservation};
    use crate::testing::paid;

    #[test]
    fn a_short_reservation_is_refused_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(2, 3)).unwrap();
        let needed = ramp(2, 3).bytes();
        let ledger = Ledger::new(usize::MAX);
        assert!(
            s.audio(id, &ledger.reserve(needed).unwrap())
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            s.audio(id, &ledger.reserve(needed - 1).unwrap()).unwrap_err(),
            StorageError::ShortReservation { id: i, source: ShortReservation { needed: n, reserved: r } }
                if i == id && n == needed && r == needed - 1
        ));
    }

    fn ramp(channels: u16, frames: i16) -> AudioBuffer {
        let data: Vec<i16> = (0..frames * channels as i16)
            .map(|i| i * 101 - 700)
            .collect();
        AudioBuffer::new(channels, data).unwrap()
    }

    fn created(dir: &Path) -> FileStorage {
        FileStorage::open(dir).unwrap()
    }

    /// Reopens the directory, as the next run of the engine would.
    fn reopen(dir: &Path) -> Result<FileStorage, StorageError> {
        FileStorage::open(dir)
    }

    #[test]
    fn open_on_an_empty_directory_creates_an_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let s = FileStorage::open(dir.path()).unwrap();
        assert!(dir.path().join(MANIFEST).exists());
        assert!(dir.path().join(SAMPLES_DIR).is_dir());
        assert!(s.sample(SampleId(0)).unwrap().is_none());
    }

    #[test]
    fn open_refuses_a_directory_with_files_that_are_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        assert!(matches!(
            FileStorage::open(dir.path()),
            Err(StorageError::NotAProject(_))
        ));
        // Nothing was created beside the stranger's files.
        assert!(!dir.path().join(MANIFEST).exists());
    }

    #[test]
    fn open_on_a_missing_directory_creates_it() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("project");
        FileStorage::open(&dir).unwrap();
        assert!(dir.join(MANIFEST).exists());
    }

    #[test]
    fn samples_and_audio_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let root = s.add_sample(None, &ramp(2, 16)).unwrap();
        let rev = s.add_sample(Some(root), &ramp(2, 8)).unwrap();
        let back = s.add_sample(Some(rev), &ramp(2, 16)).unwrap();
        drop(s);

        let s = reopen(dir.path()).unwrap();
        assert_eq!(s.sample(rev).unwrap().unwrap().parent, Some(root));
        assert_eq!(s.sample(back).unwrap().unwrap().parent, Some(rev));
        assert_eq!(s.audio(root, &paid()).unwrap().unwrap(), ramp(2, 16));
        assert_eq!(s.audio(rev, &paid()).unwrap().unwrap(), ramp(2, 8));
        assert_eq!(s.audio(back, &paid()).unwrap().unwrap(), ramp(2, 16));
    }

    #[test]
    fn unknown_id_is_none_for_sample_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let s = created(dir.path());
        assert!(s.sample(SampleId(9)).unwrap().is_none());
        assert!(s.audio(SampleId(9), &paid()).unwrap().is_none());
    }

    #[test]
    fn ids_are_not_reused_across_remove_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let a = s.add_sample(None, &ramp(1, 1)).unwrap();
        s.remove_sample(a).unwrap();
        drop(s);
        let mut s = reopen(dir.path()).unwrap();
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
    fn renamed_wav_is_an_error_for_the_record_and_the_audio() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(1, 4)).unwrap();
        fs::rename(s.wav_path(id), dir.path().join("samples/kick.wav")).unwrap();
        // The record takes the audio's shape from the WAV header, so it
        // cannot be read either; existence can, from the manifest.
        assert!(matches!(s.sample(id), Err(StorageError::Backend(_))));
        assert!(matches!(
            s.audio(id, &paid()),
            Err(StorageError::Backend(_))
        ));
        assert!(s.has_sample(id).unwrap());
    }

    #[test]
    fn the_record_carries_the_audios_shape() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(2, 3)).unwrap();
        let sample = s.sample(id).unwrap().unwrap();
        assert_eq!(sample.channels, 2);
        assert_eq!(sample.frames, 3);
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
            s.audio(id, &paid()),
            Err(StorageError::BadAudioFormat { id: i, .. }) if i == id
        ));
    }

    #[test]
    fn unsupported_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let s = created(dir.path());
        let manifest = r#"{"version":99,"next_id":0,"samples":{}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);
        assert!(matches!(
            reopen(dir.path()),
            Err(StorageError::UnsupportedVersion(99))
        ));
    }

    /// A project that was written by the engine passes the open check.
    #[test]
    fn open_accepts_a_whole_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let root = s.add_sample(None, &ramp(2, 4)).unwrap();
        s.add_sample(Some(root), &ramp(1, 4)).unwrap();
        drop(s);
        reopen(dir.path()).unwrap();
    }

    #[test]
    fn open_refuses_a_dangling_parent() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let root = s.add_sample(None, &ramp(1, 1)).unwrap();
        s.add_sample(Some(root), &ramp(1, 1)).unwrap();
        // Forget the root but leave the child pointing at it.
        let manifest = r#"{"version":1,"next_id":2,"samples":{"1":{"parent":0}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);
        assert!(matches!(reopen(dir.path()), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn open_refuses_a_parent_minted_after_its_child() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(None, &ramp(1, 1)).unwrap();
        s.add_sample(None, &ramp(1, 1)).unwrap();
        let manifest =
            r#"{"version":1,"next_id":2,"samples":{"0":{"parent":1},"1":{"parent":null}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);
        assert!(matches!(reopen(dir.path()), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn a_new_sample_starts_unloaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(1, 1)).unwrap();
        assert!(!s.sample(id).unwrap().unwrap().loaded);
    }

    #[test]
    fn set_loaded_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let a = s.add_sample(None, &ramp(1, 1)).unwrap();
        let b = s.add_sample(None, &ramp(1, 1)).unwrap();
        s.set_loaded(a, true).unwrap();
        s.set_loaded(b, true).unwrap();
        s.set_loaded(b, false).unwrap();
        // Not stored: not an error, nothing changes.
        s.set_loaded(SampleId(99), true).unwrap();
        drop(s);

        let s = reopen(dir.path()).unwrap();
        assert!(s.sample(a).unwrap().unwrap().loaded);
        assert!(!s.sample(b).unwrap().unwrap().loaded);
        assert!(s.sample(SampleId(99)).unwrap().is_none());
        assert_eq!(s.loaded_samples().unwrap(), vec![a]);
    }

    #[test]
    fn loaded_is_read_from_the_manifest_and_absent_means_unloaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(None, &ramp(1, 1)).unwrap();
        s.add_sample(None, &ramp(1, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":2,"samples":{"0":{"parent":null,"loaded":true},"1":{"parent":null}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);

        let s = reopen(dir.path()).unwrap();
        assert!(s.sample(SampleId(0)).unwrap().unwrap().loaded);
        assert!(!s.sample(SampleId(1)).unwrap().unwrap().loaded);
    }

    #[test]
    fn open_refuses_a_counter_that_is_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(None, &ramp(1, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":0,"samples":{"0":{"parent":null}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);
        assert!(matches!(reopen(dir.path()), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn open_refuses_a_missing_or_wrong_format_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(None, &ramp(1, 4)).unwrap();
        let path = s.wav_path(id);
        fs::rename(&path, dir.path().join("samples/kick.wav")).unwrap();
        assert!(matches!(reopen(dir.path()), Err(StorageError::Backend(_))));

        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        drop(s);
        assert!(matches!(
            reopen(dir.path()),
            Err(StorageError::BadAudioFormat { .. })
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
