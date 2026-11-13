//! A project: everything the device keeps for one piece of work.
//!
//! [`Project`] is a directory and keeps no copy of it: every operation
//! reads the manifest, and every write is synced before the call returns, a
//! WAV before the manifest names it, so there is no save step. A load is
//! paid for from the caller's `AudioMemory`, as a file read fills the
//! caller's buffer.
//!
//! ```text
//! myproject/
//!   project.json      the manifest: format version, id counter, samples
//!   samples/
//!     0.wav           one 48 kHz 32-bit float WAV per sample, named by id
//! ```
//!
//! WAV rather than raw bytes so any audio tool opens a sample. A 16-bit
//! WAV from the old format is refused, yet the version stays 1: nothing
//! written then was meant to be kept.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::{Audio, AudioBuffer, AudioMemory, OverBudgetError};
use crate::config::{MAX_CHANNELS, Pcm, SAMPLE_RATE};
use crate::layout::ChannelLayout;

const BITS_PER_SAMPLE: u16 = (size_of::<Pcm>() * 8) as u16;

const MANIFEST: &str = "project.json";
const SAMPLES_DIR: &str = "samples";
const FORMAT_VERSION: u32 = 1;

/// Minted when a sample is added, never reused, and meaningless in
/// another project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("no sample with id {}", .0.0)]
    UnknownSample(SampleId),
    /// Nothing was read.
    #[error(transparent)]
    OverBudget(#[from] OverBudgetError),
    #[error("not a project: {0}")]
    NotAProject(String),
    #[error("project is corrupt: {0}")]
    Corrupt(String),
    #[error("project format version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("audio for sample {} is not in the engine's format: {detail}", .id.0)]
    BadAudioFormat { id: SampleId, detail: String },
    /// A file could not be read or written: the filesystem, the WAV codec
    /// or the manifest's JSON.
    #[error(transparent)]
    File(Box<dyn std::error::Error + Send + Sync>),
}

/// Always a whole project, so no operation asks whether one is there.
#[derive(Debug)]
pub struct Project {
    dir: PathBuf,
}

impl Project {
    /// The project at `dir`, created if the directory is empty or missing,
    /// validated otherwise, and refused if it holds anything else. Nothing
    /// is loaded.
    pub fn open(dir: &Path) -> Result<Self, ProjectError> {
        let project = Self {
            dir: dir.to_path_buf(),
        };
        if project.manifest_path().exists() {
            project.validate()?;
        } else if project.is_empty_or_missing()? {
            project.create()?;
        } else {
            return Err(ProjectError::NotAProject(format!(
                "{} has files but no {MANIFEST}",
                dir.display()
            )));
        }
        Ok(project)
    }

    /// Each load is its own copy, so loading twice costs the memory twice.
    pub fn load(&self, id: SampleId, memory: &AudioMemory) -> Result<Audio, ProjectError> {
        if !self.has_sample(id)? {
            return Err(ProjectError::UnknownSample(id));
        }
        let wav = self.open_wav(id)?;
        let mut buffer = memory.alloc_buffer(wav.layout, wav.frames)?;
        self.fill(id, wav, &mut buffer)?;
        Ok(Audio::new(buffer))
    }

    /// Stores a copy of `audio` as a new sample.
    pub fn add_sample(&mut self, audio: &Audio) -> Result<SampleId, ProjectError> {
        let mut manifest = self.read_manifest()?;
        let id = SampleId(manifest.next_id);
        manifest.next_id += 1;
        self.write_wav(id, audio)?;
        manifest.samples.insert(id.0, Entry {});
        self.write_manifest(&manifest)?;
        Ok(id)
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

    /// From the manifest alone, so true even when the WAV cannot be read.
    fn has_sample(&self, id: SampleId) -> Result<bool, ProjectError> {
        Ok(self.read_manifest()?.samples.contains_key(&id.0))
    }

    fn is_empty_or_missing(&self) -> Result<bool, ProjectError> {
        match fs::read_dir(&self.dir) {
            Ok(mut entries) => Ok(entries.next().is_none()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(at(&self.dir, e)),
        }
    }

    fn create(&self) -> Result<(), ProjectError> {
        let samples = self.samples_dir();
        fs::create_dir_all(&samples).map_err(|e| at(&samples, e))?;
        self.write_manifest(&Manifest {
            version: FORMAT_VERSION,
            next_id: 0,
            samples: BTreeMap::new(),
        })
    }

    /// The manifest reads, the counter is past every id, and every WAV is
    /// present in the crate's format.
    fn validate(&self) -> Result<(), ProjectError> {
        let manifest = self.read_manifest()?;
        for &id in manifest.samples.keys() {
            self.open_wav(SampleId(id))?;
        }
        if let Some(&max) = manifest.samples.keys().next_back()
            && manifest.next_id <= max
        {
            return Err(ProjectError::Corrupt(format!(
                "next id {} does not follow the largest sample id {max}",
                manifest.next_id
            )));
        }
        Ok(())
    }

    fn read_manifest(&self) -> Result<Manifest, ProjectError> {
        let path = self.manifest_path();
        let bytes = fs::read(&path).map_err(|e| at(&path, e))?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| at(&path, e))?;
        if manifest.version != FORMAT_VERSION {
            return Err(ProjectError::UnsupportedVersion(manifest.version));
        }
        Ok(manifest)
    }

    /// Through a synced temp file and rename, so a power cut leaves the old
    /// manifest or the new one.
    fn write_manifest(&self, manifest: &Manifest) -> Result<(), ProjectError> {
        let path = self.manifest_path();
        let json = serde_json::to_vec_pretty(manifest).map_err(|e| at(&path, e))?;
        let tmp = self.dir.join(format!("{MANIFEST}.tmp"));
        write_synced(&tmp, &json)?;
        fs::rename(&tmp, &path).map_err(|e| at(&path, e))?;
        sync_dir(&self.dir)
    }

    fn write_wav(&self, id: SampleId, audio: &Audio) -> Result<(), ProjectError> {
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

    fn open_wav(&self, id: SampleId) -> Result<Wav, ProjectError> {
        let path = self.wav_path(id);
        let reader = WavReader::open(&path).map_err(|e| at(&path, e))?;
        let spec = reader.spec();
        let layout = ChannelLayout::try_from(spec.channels)
            .ok()
            .filter(|_| {
                spec.sample_rate == SAMPLE_RATE
                    && spec.bits_per_sample == BITS_PER_SAMPLE
                    && spec.sample_format == SampleFormat::Float
            })
            .ok_or_else(|| ProjectError::BadAudioFormat {
                id,
                detail: format!("{spec:?}"),
            })?;
        // Whole frames: hound refuses to open a WAV that ends mid-frame.
        let frames = reader.duration() as usize;
        Ok(Wav {
            reader,
            layout,
            frames,
        })
    }

    /// A frame at a time, so `buffer` only ever takes whole frames. It must
    /// have room for all of them.
    fn fill(&self, id: SampleId, wav: Wav, buffer: &mut AudioBuffer) -> Result<(), ProjectError> {
        let path = self.wav_path(id);
        let c = wav.layout.count();
        let mut reader = wav.reader;
        let mut frame = [0.0; MAX_CHANNELS as usize];
        for (i, sample) in reader.samples::<Pcm>().enumerate() {
            frame[i % c] = sample.map_err(|e| at(&path, e))?;
            if i % c == c - 1 {
                buffer.extend(frame[..c].iter().copied());
            }
        }
        Ok(())
    }
}

/// Everything but the audio. Keyed by id, so an id cannot appear twice and
/// the file lists samples in order.
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    next_id: u32,
    samples: BTreeMap<u32, Entry>,
}

/// What the manifest keeps about a sample; empty until prints bring back
/// the parent pointer. A new field needs `#[serde(default)]` so older
/// manifests still open; unknown keys are ignored, so the old `parent` and
/// `loaded` fields are too.
#[derive(Serialize, Deserialize)]
struct Entry {}

/// A WAV whose header is checked and no audio read: its shape is known
/// before any memory is taken for it.
struct Wav {
    reader: WavReader<BufReader<File>>,
    layout: ChannelLayout,
    frames: usize,
}

#[derive(Debug, Error)]
#[error("{path}: {source}")]
struct FileError {
    path: PathBuf,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

/// A file error carrying the path it happened at.
fn at(path: &Path, source: impl std::error::Error + Send + Sync + 'static) -> ProjectError {
    ProjectError::File(Box::new(FileError {
        path: path.to_path_buf(),
        source: Box::new(source),
    }))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
    use std::io::Write;
    let mut file = File::create(path).map_err(|e| at(path, e))?;
    file.write_all(bytes).map_err(|e| at(path, e))?;
    file.sync_all().map_err(|e| at(path, e))
}

/// So a file created or renamed in it survives a power cut.
fn sync_dir(dir: &Path) -> Result<(), ProjectError> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| at(dir, e))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::mem::size_of;

    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::TempDir;

    use super::*;
    use crate::config::{Pcm, SAMPLE_RATE};
    use crate::layout::ChannelLayout::{self, Mono, Stereo};

    /// Dropping the directory deletes the project.
    fn project() -> (TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::open(dir.path()).unwrap();
        (dir, p)
    }

    /// As the next run of the sampler would.
    fn reopen(dir: &TempDir) -> Result<Project, ProjectError> {
        Project::open(dir.path())
    }

    /// `frames` of `layout`, no two samples alike.
    fn ramp(layout: ChannelLayout, frames: usize) -> Audio {
        let mut audio = AudioMemory::new().alloc(layout, frames).unwrap();
        audio
            .extend((0..frames * layout.count()).map(|i| i as Pcm * 0.101 - 0.7))
            .unwrap();
        audio
    }

    /// What `ramp(layout, frames)` costs the budget.
    fn ramp_bytes(layout: ChannelLayout, frames: usize) -> usize {
        frames * layout.count() * size_of::<Pcm>()
    }

    fn assert_same(a: &Audio, b: &Audio) {
        assert_eq!(a.layout(), b.layout());
        assert_eq!(a.samples(), b.samples());
    }

    /// Overwrites the WAV at `path` with one sample in `spec`'s format.
    fn write_wav(path: &Path, spec: WavSpec) {
        let mut w = WavWriter::create(path, spec).unwrap();
        match spec.sample_format {
            SampleFormat::Float => w.write_sample(0.0 as Pcm).unwrap(),
            SampleFormat::Int => w.write_sample(0i16).unwrap(),
        }
        w.finalize().unwrap();
    }

    #[test]
    fn open_on_an_empty_directory_creates_an_empty_project() {
        let (dir, p) = project();
        assert!(dir.path().join(MANIFEST).exists());
        assert!(dir.path().join(SAMPLES_DIR).is_dir());
        assert!(!p.has_sample(SampleId(0)).unwrap());
    }

    #[test]
    fn open_on_a_missing_directory_creates_it() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("project");
        Project::open(&dir).unwrap();
        assert!(dir.join(MANIFEST).exists());
    }

    #[test]
    fn open_refuses_a_directory_with_files_that_are_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hi").unwrap();
        assert!(matches!(
            Project::open(dir.path()),
            Err(ProjectError::NotAProject(_))
        ));
        // Nothing was created beside the stranger's files.
        assert!(!dir.path().join(MANIFEST).exists());
    }

    #[test]
    fn add_stores_a_sample_under_a_new_id() {
        let (_dir, mut p) = project();
        let a = p.add_sample(&ramp(Mono, 4)).unwrap();
        let b = p.add_sample(&ramp(Mono, 4)).unwrap();
        assert_ne!(a, b);
        assert!(p.has_sample(a).unwrap());
        assert!(p.has_sample(b).unwrap());
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let (dir, mut p) = project();
        p.add_sample(&ramp(Mono, 1)).unwrap();
        assert!(!dir.path().join("project.json.tmp").exists());
    }

    #[test]
    fn load_returns_the_audio_paid_for_exactly() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&ramp(Stereo, 3)).unwrap();

        let loaded = p.load(id, &memory).unwrap();
        assert_same(&loaded, &ramp(Stereo, 3));
        assert_eq!(memory.in_use(), ramp_bytes(Stereo, 3));
    }

    #[test]
    fn dropping_loaded_audio_unloads_it() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&ramp(Mono, 4)).unwrap();
        let loaded = p.load(id, &memory).unwrap();

        drop(loaded);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn each_load_is_its_own_copy() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let id = p.add_sample(&ramp(Mono, 4)).unwrap();
        let a = p.load(id, &memory).unwrap();
        let b = p.load(id, &memory).unwrap();
        assert_eq!(memory.in_use(), 2 * ramp_bytes(Mono, 4));
        drop(a);
        assert_eq!(memory.in_use(), ramp_bytes(Mono, 4));
        drop(b);
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn load_over_budget_is_refused_before_any_audio_is_read() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::with_budget(ramp_bytes(Stereo, 3));
        let a = p.add_sample(&ramp(Stereo, 3)).unwrap();
        let b = p.add_sample(&ramp(Stereo, 3)).unwrap();
        let _held = p.load(a, &memory).unwrap();

        assert!(matches!(
            p.load(b, &memory),
            Err(ProjectError::OverBudget(e)) if e.requested == ramp_bytes(Stereo, 3)
        ));
        assert_eq!(memory.in_use(), ramp_bytes(Stereo, 3));
    }

    #[test]
    fn load_of_an_unknown_sample_is_refused() {
        let (_dir, mut p) = project();
        let memory = AudioMemory::new();
        let root = p.add_sample(&ramp(Mono, 4)).unwrap();
        let bogus = SampleId(root.0 + 1);
        assert!(matches!(
            p.load(bogus, &memory),
            Err(ProjectError::UnknownSample(id)) if id == bogus
        ));
        assert_eq!(memory.in_use(), 0);
    }

    #[test]
    fn samples_survive_a_reopen() {
        let (dir, mut p) = project();
        let a = p.add_sample(&ramp(Stereo, 16)).unwrap();
        let b = p.add_sample(&ramp(Mono, 8)).unwrap();
        drop(p);

        let p = reopen(&dir).unwrap();
        let memory = AudioMemory::new();
        assert_same(&p.load(a, &memory).unwrap(), &ramp(Stereo, 16));
        assert_same(&p.load(b, &memory).unwrap(), &ramp(Mono, 8));
    }

    #[test]
    fn a_renamed_wav_fails_the_load_only() {
        let (dir, mut p) = project();
        let id = p.add_sample(&ramp(Mono, 4)).unwrap();
        fs::rename(p.wav_path(id), dir.path().join("samples/kick.wav")).unwrap();
        // Existence comes from the manifest; only the audio needs the file.
        assert!(matches!(
            p.load(id, &AudioMemory::new()),
            Err(ProjectError::File(_))
        ));
        assert!(p.has_sample(id).unwrap());
    }

    #[test]
    fn the_wrong_sample_rate_or_a_16_bit_wav_is_the_wrong_format() {
        let (_dir, mut p) = project();
        let id = p.add_sample(&ramp(Mono, 4)).unwrap();
        for spec in [
            WavSpec {
                channels: 1,
                sample_rate: 44_100,
                bits_per_sample: BITS_PER_SAMPLE,
                sample_format: SampleFormat::Float,
            },
            WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        ] {
            write_wav(&p.wav_path(id), spec);
            assert!(matches!(
                p.load(id, &AudioMemory::new()),
                Err(ProjectError::BadAudioFormat { id: i, .. }) if i == id
            ));
        }
    }

    #[test]
    fn open_accepts_a_whole_project() {
        let (dir, mut p) = project();
        p.add_sample(&ramp(Stereo, 4)).unwrap();
        p.add_sample(&ramp(Mono, 4)).unwrap();
        drop(p);
        reopen(&dir).unwrap();
    }

    #[test]
    fn open_refuses_an_unsupported_version() {
        let (dir, p) = project();
        let manifest = r#"{"version":99,"next_id":0,"samples":{}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(p);
        assert!(matches!(
            reopen(&dir),
            Err(ProjectError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn a_manifest_with_old_fields_still_opens() {
        // Older manifests carry `parent` and `loaded` per entry.
        let (dir, mut p) = project();
        p.add_sample(&ramp(Mono, 1)).unwrap();
        p.add_sample(&ramp(Mono, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":2,"samples":{"0":{"parent":null,"loaded":true},"1":{"parent":null}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(p);

        let p = reopen(&dir).unwrap();
        assert!(p.has_sample(SampleId(0)).unwrap());
        assert!(p.has_sample(SampleId(1)).unwrap());
    }

    #[test]
    fn open_refuses_a_counter_that_is_behind() {
        let (dir, mut p) = project();
        p.add_sample(&ramp(Mono, 1)).unwrap();
        let manifest = r#"{"version":1,"next_id":0,"samples":{"0":{}}}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        drop(p);
        assert!(matches!(reopen(&dir), Err(ProjectError::Corrupt(_))));
    }

    #[test]
    fn open_refuses_a_missing_or_wrong_format_wav() {
        let (dir, mut p) = project();
        let id = p.add_sample(&ramp(Mono, 4)).unwrap();
        let path = p.wav_path(id);
        drop(p);
        fs::rename(&path, dir.path().join("samples/kick.wav")).unwrap();
        assert!(matches!(reopen(&dir), Err(ProjectError::File(_))));

        write_wav(
            &path,
            WavSpec {
                channels: 1,
                sample_rate: 44_100,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        );
        assert!(matches!(
            reopen(&dir),
            Err(ProjectError::BadAudioFormat { .. })
        ));
    }

    #[test]
    fn open_refuses_a_wav_that_ends_mid_frame() {
        let (dir, mut p) = project();
        let id = p.add_sample(&ramp(Stereo, 2)).unwrap();
        let path = p.wav_path(id);
        drop(p);
        let spec = WavSpec {
            channels: 2,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: BITS_PER_SAMPLE,
            sample_format: SampleFormat::Float,
        };
        let mut w = WavWriter::create(&path, spec).unwrap();
        for _ in 0..3 {
            w.write_sample(0.0 as Pcm).unwrap();
        }
        // Writes the header and then reports the part frame it just wrote.
        assert!(w.finalize().is_err());

        // hound refuses it before the project looks.
        assert!(matches!(reopen(&dir), Err(ProjectError::File(_))));
    }
}
