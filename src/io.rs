//! Save and load: a project as a self-contained directory.
//!
//! ```text
//! myproject/
//!   project.json      the manifest: format version, id counter, lineage
//!   samples/
//!     0.wav           one 48 kHz 16-bit WAV per sample, named by id
//!     1.wav
//! ```
//!
//! The manifest is a separate, private struct rather than a serde derive on
//! [`Project`], so the file format is versioned on its own and the model
//! knows nothing about disk. Audio does not go through serde at all: a WAV
//! is the buffer's bytes behind a header that any audio tool can open.
//!
//! The directory is engine-owned. Load is strict: a missing WAV, a dangling
//! parent or a WAV in the wrong format is an error, not a dropped sample,
//! so a project that opens has its lineage intact. Renaming a WAV by hand
//! breaks the project.
//!
//! Samples never change and ids are never reused, so a WAV named for an id
//! never needs rewriting. Save writes only the WAVs that are missing, then
//! replaces the manifest atomically, then removes WAVs for ids no longer in
//! the project. A crash leaves the old or the new manifest, and a stray WAV
//! is ignored on load.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError};
use crate::config::{MAX_CHANNELS, Pcm, SAMPLE_RATE};
use crate::project::{Project, ProjectError};
use crate::sample::SampleId;

const MANIFEST: &str = "project.json";
const SAMPLES_DIR: &str = "samples";
const FORMAT_VERSION: u32 = 1;

/// Reasons a project cannot be saved or loaded.
#[derive(Debug, Error)]
pub enum IoError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: {source}")]
    Wav {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    /// The manifest was written by a format this build does not read.
    #[error("manifest format version {0} is not supported (this build reads {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    /// A WAV is not in the engine's one fixed format.
    #[error(
        "{path}: {spec:?} is not {SAMPLE_RATE} Hz, {bits}-bit PCM, 1..={MAX_CHANNELS} channels",
        bits = Pcm::BITS
    )]
    BadFormat { path: PathBuf, spec: WavSpec },
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Project(#[from] ProjectError),
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

/// Writes `project` to `dir`, creating it if needed.
pub fn save(project: &Project, dir: &Path) -> Result<(), IoError> {
    let samples_dir = dir.join(SAMPLES_DIR);
    fs::create_dir_all(&samples_dir).map_err(|e| io_err(&samples_dir, e))?;

    let mut entries: Vec<Entry> = Vec::new();
    for s in project.iter() {
        let path = wav_path(dir, s.id());
        if !path.exists() {
            write_wav(&path, s.audio())?;
        }
        entries.push(Entry {
            id: s.id().raw(),
            parent: s.parent().map(SampleId::raw),
        });
    }
    entries.sort_by_key(|e| e.id);

    let manifest = Manifest {
        version: FORMAT_VERSION,
        next_id: project.next_id(),
        samples: entries,
    };
    let path = dir.join(MANIFEST);
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| IoError::Manifest {
        path: path.clone(),
        source: e,
    })?;
    let tmp = dir.join(format!("{MANIFEST}.tmp"));
    fs::write(&tmp, json).map_err(|e| io_err(&tmp, e))?;
    fs::rename(&tmp, &path).map_err(|e| io_err(&path, e))?;

    remove_orphans(project, &samples_dir)
}

/// Reads the project in `dir`.
pub fn load(dir: &Path) -> Result<Project, IoError> {
    let path = dir.join(MANIFEST);
    let bytes = fs::read(&path).map_err(|e| io_err(&path, e))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| IoError::Manifest {
        path: path.clone(),
        source: e,
    })?;
    if manifest.version != FORMAT_VERSION {
        return Err(IoError::UnsupportedVersion(manifest.version));
    }

    let mut samples = Vec::with_capacity(manifest.samples.len());
    for e in &manifest.samples {
        let id = SampleId(e.id);
        let audio = read_wav(&wav_path(dir, id))?;
        samples.push((id, e.parent.map(SampleId), audio));
    }
    Ok(Project::restore(samples, manifest.next_id)?)
}

fn wav_path(dir: &Path, id: SampleId) -> PathBuf {
    dir.join(SAMPLES_DIR).join(format!("{}.wav", id.raw()))
}

fn write_wav(path: &Path, audio: &AudioBuffer) -> Result<(), IoError> {
    let spec = WavSpec {
        channels: audio.channels(),
        sample_rate: SAMPLE_RATE,
        bits_per_sample: Pcm::BITS as u16,
        sample_format: SampleFormat::Int,
    };
    let wav = |e| wav_err(path, e);
    let mut writer = WavWriter::create(path, spec).map_err(wav)?;
    let mut w = writer.get_i16_writer(audio.samples().len() as u32);
    for &s in audio.samples() {
        w.write_sample(s);
    }
    w.flush().map_err(wav)?;
    writer.finalize().map_err(wav)
}

fn read_wav(path: &Path) -> Result<AudioBuffer, IoError> {
    let wav = |e| wav_err(path, e);
    let mut reader = WavReader::open(path).map_err(wav)?;
    let spec = reader.spec();
    let expected = spec.sample_rate == SAMPLE_RATE
        && spec.bits_per_sample == Pcm::BITS as u16
        && spec.sample_format == SampleFormat::Int
        && (1..=MAX_CHANNELS).contains(&spec.channels);
    if !expected {
        return Err(IoError::BadFormat {
            path: path.to_path_buf(),
            spec,
        });
    }
    let data = reader
        .samples::<Pcm>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(wav)?;
    Ok(AudioBuffer::new(spec.channels, data)?)
}

/// Deletes WAVs in `samples_dir` whose id is no longer in the project.
fn remove_orphans(project: &Project, samples_dir: &Path) -> Result<(), IoError> {
    for entry in fs::read_dir(samples_dir).map_err(|e| io_err(samples_dir, e))? {
        let path = entry.map_err(|e| io_err(samples_dir, e))?.path();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok());
        let live = id.is_some_and(|id| project.get(SampleId(id)).is_some());
        if path.extension().is_some_and(|x| x == "wav") && !live {
            fs::remove_file(&path).map_err(|e| io_err(&path, e))?;
        }
    }
    Ok(())
}

/// A hound error that is really an `io::Error` (a missing file, say) is
/// reported as one, so callers see the same error for a missing WAV as
/// for a missing manifest.
fn wav_err(path: &Path, source: hound::Error) -> IoError {
    match source {
        hound::Error::IoError(e) => io_err(path, e),
        source => IoError::Wav {
            path: path.to_path_buf(),
            source,
        },
    }
}

fn io_err(path: &Path, source: io::Error) -> IoError {
    IoError::Io {
        path: path.to_path_buf(),
        source,
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

    fn round_trip(p: &Project) -> Project {
        let dir = tempfile::tempdir().unwrap();
        save(p, dir.path()).unwrap();
        load(dir.path()).unwrap()
    }

    #[test]
    fn empty_project_round_trips() {
        let p = round_trip(&Project::new());
        assert_eq!(p.iter().count(), 0);
        assert_eq!(p.next_id(), 0);
    }

    #[test]
    fn lineage_and_audio_survive_a_round_trip() {
        let mut p = Project::new();
        let root = p.add(None, ramp(2, 16)).unwrap();
        let rev = p
            .add(Some(root), reverse(p.get(root).unwrap().audio()))
            .unwrap();
        let back = p
            .add(Some(rev), reverse(p.get(rev).unwrap().audio()))
            .unwrap();
        let branch = p.add(Some(root), ramp(1, 3)).unwrap();

        let q = round_trip(&p);
        assert_eq!(q.iter().count(), 4);
        for id in [root, rev, back, branch] {
            let (a, b) = (p.get(id).unwrap(), q.get(id).unwrap());
            assert_eq!(a.parent(), b.parent());
            assert_eq!(a.audio(), b.audio());
        }
        // Reverse twice is exact, through the disk too.
        assert_eq!(q.get(back).unwrap().audio(), q.get(root).unwrap().audio());
    }

    #[test]
    fn counter_survives_so_ids_are_not_reused_after_a_reload() {
        let mut p = Project::new();
        let a = p.add(None, ramp(1, 1)).unwrap();
        p.remove(a).unwrap();
        let mut q = round_trip(&p);
        let b = q.add(None, ramp(1, 1)).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn saving_twice_removes_wavs_of_removed_samples_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new();
        let keep = p.add(None, ramp(1, 4)).unwrap();
        let drop = p.add(None, ramp(1, 4)).unwrap();
        save(&p, dir.path()).unwrap();
        assert!(wav_path(dir.path(), drop).exists());

        p.remove(drop).unwrap();
        let added = p.add(None, ramp(1, 2)).unwrap();
        save(&p, dir.path()).unwrap();
        assert!(!wav_path(dir.path(), drop).exists());
        assert!(wav_path(dir.path(), keep).exists());
        assert!(wav_path(dir.path(), added).exists());
        assert!(!dir.path().join("project.json.tmp").exists());
        assert_eq!(load(dir.path()).unwrap().iter().count(), 2);
    }

    #[test]
    fn missing_wav_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new();
        let id = p.add(None, ramp(1, 4)).unwrap();
        save(&p, dir.path()).unwrap();
        fs::rename(
            wav_path(dir.path(), id),
            dir.path().join("samples").join("kick.wav"),
        )
        .unwrap();
        assert!(matches!(load(dir.path()), Err(IoError::Io { .. })));
    }

    #[test]
    fn dangling_parent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new();
        let root = p.add(None, ramp(1, 1)).unwrap();
        p.add(Some(root), ramp(1, 1)).unwrap();
        save(&p, dir.path()).unwrap();
        // The manifest forgets the root but the child still points at it.
        let manifest = r#"{"version":1,"next_id":2,"samples":[{"id":1,"parent":0}]}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(IoError::Project(ProjectError::UnknownSample(SampleId(0))))
        ));
    }

    #[test]
    fn unsupported_version_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = r#"{"version":99,"next_id":0,"samples":[]}"#;
        fs::write(dir.path().join(MANIFEST), manifest).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(IoError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn wrong_sample_rate_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new();
        let id = p.add(None, ramp(1, 4)).unwrap();
        save(&p, dir.path()).unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(wav_path(dir.path(), id), spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        assert!(matches!(load(dir.path()), Err(IoError::BadFormat { .. })));
    }
}
