//! Where a project is kept: a directory.
//!
//! There is no in-memory copy of a project. A [`Project`] holds its
//! storage and every read and write goes through it, so the project is
//! the disk. Storage answers questions and does what it is told; the rules
//! about what is allowed live in the project.
//!
//! ```text
//! myproject/
//!   project.json      the manifest: format version, id counter, samples
//!   samples/
//!     0.wav           one 48 kHz 32-bit float WAV per sample, named by id
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
//! manifest or the new one, never half of either. The directory is the
//! project's; renaming a WAV by hand breaks the sample, and
//! [`Storage::open`] checks the whole project before opening on it.
//!
//! The WAV holds the crate's sample type as it is, so a read is a copy
//! from disk into a block and nothing converts. A 16-bit project written
//! by `storage` is refused as the wrong format; the format version does
//! not move because nothing written at version 1 is meant to be kept. A
//! read is a [`Block`] from the ledger.
//!
//! [`Project`]: crate::project::Project
//! [`Block`]: crate::project::ledger::Block

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::config::{Pcm, SAMPLE_RATE};
use crate::project::ledger::{Ledger, OverBudgetError};
use crate::project::{SampleId, SampleMetadata};

/// Reasons storage cannot do what was asked.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The stored project was written by a format this build does not read.
    #[error("stored format version {0} is not supported")]
    UnsupportedVersion(u32),
    /// Something is stored here, and it is not a project at all.
    #[error("storage holds something that is not a project: {0}")]
    NotAProject(String),
    /// Something is stored here, but it is not a whole project.
    #[error("stored project is corrupt: {0}")]
    Corrupt(String),
    /// Stored audio is not in the crate's one fixed format.
    #[error("audio for sample {} is not in the engine's format: {detail}", .id.0)]
    BadAudioFormat { id: SampleId, detail: String },
    /// The sample's audio would put audio memory over its budget.
    /// Nothing was allocated.
    #[error("reading sample {}: {source}", .id.0)]
    OverBudget {
        id: SampleId,
        #[source]
        source: OverBudgetError,
    },
    /// Something failed underneath: the filesystem or the WAV codec.
    #[error(transparent)]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

/// Bits in one stored sample, as the WAV header counts them.
const BITS_PER_SAMPLE: u16 = (size_of::<Pcm>() * 8) as u16;

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

/// A sample's [`SampleMetadata`] as the manifest writes it; its id is the
/// map key.
///
/// Empty for now, as the metadata is: the map's keys record which samples
/// exist. A new field goes in with `#[serde(default)]` so manifests
/// written before it still open. Unknown keys are ignored, so manifests
/// that carry the old `parent` or `loaded` fields open too.
#[derive(Serialize, Deserialize)]
struct Entry {}

impl From<&SampleMetadata> for Entry {
    fn from(metadata: &SampleMetadata) -> Self {
        // Destructured so a new metadata field does not compile until it
        // is written here.
        let SampleMetadata {} = metadata;
        Entry {}
    }
}

/// A project stored as a directory. A storage always holds a whole
/// project, so its operations never ask whether one is there.
#[derive(Debug)]
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
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
    /// the counter is past every id, and every sample's WAV is present
    /// with a header in the crate's format.
    fn validate(&self) -> Result<(), StorageError> {
        let manifest = self.read_manifest()?;
        for &id in manifest.samples.keys() {
            self.open_wav(SampleId(id))?;
        }
        if let Some(&max) = manifest.samples.keys().next_back()
            && manifest.next_id <= max
        {
            return Err(StorageError::Corrupt(format!(
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
            channels: audio.layout().into(),
            sample_rate: SAMPLE_RATE,
            bits_per_sample: BITS_PER_SAMPLE,
            sample_format: SampleFormat::Float,
        };
        let mut file = File::create(&path).map_err(|e| at(&path, e))?;
        {
            let mut writer = WavWriter::new(&mut file, spec).map_err(|e| at(&path, e))?;
            for &s in audio.samples() {
                writer.write_sample(s).map_err(|e| at(&path, e))?;
            }
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

    /// Reads a sample's WAV into a block of exactly its sample count,
    /// allocated from `ledger` after the header is checked and before any
    /// audio is read.
    fn read_wav(&self, id: SampleId, ledger: &Ledger) -> Result<AudioBuffer, StorageError> {
        let path = self.wav_path(id);
        let mut reader = self.open_wav(id)?;
        let layout = ChannelLayout::try_from(reader.spec().channels).map_err(|e| at(&path, e))?;
        let mut data = ledger
            .alloc(reader.len() as usize)
            .map_err(|source| StorageError::OverBudget { id, source })?;
        for sample in reader.samples::<Pcm>() {
            data.push(sample.map_err(|e| at(&path, e))?);
        }
        AudioBuffer::new(layout, data).map_err(|(e, _)| at(&path, e))
    }
}

/// Refuses a WAV header that is not the crate's one fixed format.
fn check_spec(id: SampleId, spec: WavSpec) -> Result<(), StorageError> {
    let expected = spec.sample_rate == SAMPLE_RATE
        && spec.bits_per_sample == BITS_PER_SAMPLE
        && spec.sample_format == SampleFormat::Float
        && ChannelLayout::try_from(spec.channels).is_ok();
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

impl Storage {
    /// Whether a sample with this id is stored. Answered from the record
    /// alone, so it holds even when the sample's audio cannot be read.
    pub fn has_sample(&self, id: SampleId) -> Result<bool, StorageError> {
        Ok(self.read_manifest()?.samples.contains_key(&id.0))
    }

    /// The stored audio of the sample with this id in a block from
    /// `ledger`, or `None` if there is none.
    ///
    /// The block is sized from what is stored before any audio is read,
    /// so a sample that does not fit is refused with nothing allocated.
    pub fn audio(
        &self,
        id: SampleId,
        ledger: &Ledger,
    ) -> Result<Option<AudioBuffer>, StorageError> {
        if !self.has_sample(id)? {
            return Ok(None);
        }
        self.read_wav(id, ledger).map(Some)
    }

    /// Stores a sample, its metadata beside its audio, and returns its new
    /// id. Ids come from a counter that only goes up, so an id is never
    /// reused.
    pub fn add_sample(
        &mut self,
        metadata: &SampleMetadata,
        audio: &AudioBuffer,
    ) -> Result<SampleId, StorageError> {
        let mut manifest = self.read_manifest()?;
        let id = SampleId(manifest.next_id);
        manifest.next_id += 1;
        self.write_wav(id, audio)?;
        manifest.samples.insert(id.0, metadata.into());
        self.write_manifest(&manifest)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ChannelLayout::{Mono, Stereo};
    use crate::testing::unlimited;

    #[test]
    fn a_read_is_paid_for_exactly_and_returned_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Stereo, 3)).unwrap();
        let bytes = 6 * size_of::<Pcm>();
        let ledger = Ledger::new(bytes);
        let audio = s.audio(id, &ledger).unwrap().unwrap();
        assert_eq!(ledger.in_use(), bytes);
        drop(audio);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn over_budget_is_refused_with_nothing_allocated() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Stereo, 3)).unwrap();
        let bytes = 6 * size_of::<Pcm>();
        let ledger = Ledger::new(bytes - 1);
        assert!(matches!(
            s.audio(id, &ledger).unwrap_err(),
            StorageError::OverBudget { id: i, source } if i == id && source.requested == bytes
        ));
        assert_eq!(ledger.in_use(), 0);
    }

    fn ramp(layout: ChannelLayout, frames: usize) -> AudioBuffer {
        let mut block = unlimited().alloc(frames * layout.count()).unwrap();
        block.extend((0..block.capacity()).map(|i| i as Pcm * 0.101 - 0.7));
        AudioBuffer::new(layout, block).unwrap()
    }

    fn assert_same(a: &AudioBuffer, b: &AudioBuffer) {
        assert_eq!(a.layout(), b.layout());
        assert_eq!(a.samples(), b.samples());
    }

    fn created(dir: &Path) -> Storage {
        Storage::open(dir).unwrap()
    }

    /// Reopens the directory, as the next run of the sampler would.
    fn reopen(dir: &Path) -> Result<Storage, StorageError> {
        Storage::open(dir)
    }

    #[test]
    fn open_on_an_empty_directory_creates_an_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::open(dir.path()).unwrap();
        assert!(dir.path().join(MANIFEST).exists());
        assert!(dir.path().join(SAMPLES_DIR).is_dir());
        assert!(!s.has_sample(SampleId(0)).unwrap());
    }

    #[test]
    fn open_refuses_a_directory_with_files_that_are_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        assert!(matches!(
            Storage::open(dir.path()),
            Err(StorageError::NotAProject(_))
        ));
        // Nothing was created beside the stranger's files.
        assert!(!dir.path().join(MANIFEST).exists());
    }

    #[test]
    fn open_on_a_missing_directory_creates_it() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("project");
        Storage::open(&dir).unwrap();
        assert!(dir.join(MANIFEST).exists());
    }

    #[test]
    fn samples_and_audio_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let a = s.add_sample(&SampleMetadata {}, &ramp(Stereo, 16)).unwrap();
        let b = s.add_sample(&SampleMetadata {}, &ramp(Mono, 8)).unwrap();
        drop(s);

        let s = reopen(dir.path()).unwrap();
        assert_same(
            &s.audio(a, &unlimited()).unwrap().unwrap(),
            &ramp(Stereo, 16),
        );
        assert_same(&s.audio(b, &unlimited()).unwrap().unwrap(), &ramp(Mono, 8));
    }

    #[test]
    fn unknown_id_is_none_for_sample_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let s = created(dir.path());
        assert!(!s.has_sample(SampleId(9)).unwrap());
        assert!(s.audio(SampleId(9), &unlimited()).unwrap().is_none());
    }

    #[test]
    fn renamed_wav_is_an_error_for_the_audio_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Mono, 4)).unwrap();
        fs::rename(s.wav_path(id), dir.path().join("samples/kick.wav")).unwrap();
        // Existence comes from the manifest; only the audio needs the file.
        assert!(matches!(
            s.audio(id, &unlimited()),
            Err(StorageError::Backend(_))
        ));
        assert!(s.has_sample(id).unwrap());
    }

    #[test]
    fn wrong_sample_rate_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Mono, 4)).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: BITS_PER_SAMPLE,
            sample_format: SampleFormat::Float,
        };
        let mut w = WavWriter::create(s.wav_path(id), spec).unwrap();
        w.write_sample(0.0 as Pcm).unwrap();
        w.finalize().unwrap();
        assert!(matches!(
            s.audio(id, &unlimited()),
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

    /// A project that was written by this crate passes the open check.
    #[test]
    fn open_accepts_a_whole_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(&SampleMetadata {}, &ramp(Stereo, 4)).unwrap();
        s.add_sample(&SampleMetadata {}, &ramp(Mono, 4)).unwrap();
        drop(s);
        reopen(dir.path()).unwrap();
    }

    #[test]
    fn a_manifest_with_old_fields_still_opens() {
        // Manifests written while samples had a `parent` or a `loaded`
        // flag carry them per entry. Nothing reads them now; they are
        // ignored, not refused, so the format version does not move.
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(&SampleMetadata {}, &ramp(Mono, 1)).unwrap();
        s.add_sample(&SampleMetadata {}, &ramp(Mono, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":2,"samples":{"0":{"parent":null,"loaded":true},"1":{"parent":null}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);

        let s = reopen(dir.path()).unwrap();
        assert!(s.has_sample(SampleId(0)).unwrap());
        assert!(s.has_sample(SampleId(1)).unwrap());
    }

    #[test]
    fn open_refuses_a_counter_that_is_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        s.add_sample(&SampleMetadata {}, &ramp(Mono, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":0,"samples":{"0":{}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(s);
        assert!(matches!(reopen(dir.path()), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn a_16_bit_wav_is_the_wrong_format() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Mono, 4)).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(s.wav_path(id), spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        assert!(matches!(
            s.audio(id, &unlimited()),
            Err(StorageError::BadAudioFormat { id: i, .. }) if i == id
        ));
    }

    #[test]
    fn open_refuses_a_missing_or_wrong_format_wav() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = created(dir.path());
        let id = s.add_sample(&SampleMetadata {}, &ramp(Mono, 4)).unwrap();
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
        s.add_sample(&SampleMetadata {}, &ramp(Mono, 1)).unwrap();
        assert!(!dir.path().join("project.json.tmp").exists());
    }
}
