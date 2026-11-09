//! Reading audio files into samples, paid for against the ledger.
//!
//! One function, [`decode`], takes a path and returns a [`Decoded`]: the
//! file's audio as interleaved [`Pcm`] samples at the file's own rate
//! and channel count, in a [`Block`] the decoder paid for itself. The one
//! format read is WAV: RIFF/WAVE with integer PCM at 8 to 32 bits, or
//! 32-bit float. A second format brings its own dispatch when it comes.
//!
//! A decoder knows the engine's sample type and nothing else about the
//! engine's format: every width a file can hold lands in that type,
//! scaled to the same range, so the only thing left for `convert` to
//! notice is the rate. The decoder reads the sample count from the file's
//! header and asks the ledger for exactly that before it reads any audio,
//! so a file that does not fit is refused with nothing allocated.

use std::io;
use std::path::Path;
use std::sync::Arc;

use hound::{SampleFormat, WavReader};
use thiserror::Error;

use crate::config::Pcm;
use crate::memory::{Block, Ledger, OverBudget};

/// Audio as a decoder found it: the engine's sample type at the file's
/// own rate and channel count.
#[derive(Debug)]
pub struct Decoded {
    /// Sample rate of the source, in Hz.
    pub rate: u32,
    /// Interleaved channel count of the source.
    pub channels: u16,
    /// Interleaved samples, `channels` per frame, paid for by the decoder.
    pub samples: Block<Pcm>,
}

/// Reasons a file could not be read into samples.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("reading {path}: {source}")]
    Io {
        path: Box<Path>,
        #[source]
        source: io::Error,
    },
    /// The file is not a WAV this decoder reads: another format, a broken
    /// one, or a sample width it does not handle.
    #[error("{path}: {reason}")]
    Malformed { path: Box<Path>, reason: String },
    /// The file's samples would put the engine's memory over its budget.
    /// Nothing was allocated.
    #[error("{path}: {source}")]
    OverBudget {
        path: Box<Path>,
        #[source]
        source: OverBudget,
    },
}

/// Reads a WAV file into a block of exactly its sample count, allocated
/// from `ledger` after the header is checked and before any audio is
/// read, so a refused file costs an open and a header.
///
/// Floats are taken as they are; integers are scaled by their own width
/// so each lands on the same range, which is exact up to 24 bits.
/// Anything else `hound` can open but we do not expect is reported as
/// malformed rather than guessed at.
pub fn decode(path: &Path, ledger: &Arc<Ledger>) -> Result<Decoded, DecodeError> {
    let mut reader = WavReader::open(path).map_err(|e| hound_error(path, e))?;
    let spec = reader.spec();
    let width = check_width(path, spec)?;
    let mut samples =
        ledger
            .alloc(reader.len() as usize)
            .map_err(|source| DecodeError::OverBudget {
                path: path.into(),
                source,
            })?;
    match width {
        Width::Float => fill(path, &mut samples, reader.samples::<f32>())?,
        Width::Int(bits) => {
            let scale = (1u64 << (bits - 1)) as f32;
            let scaled = reader.samples::<i32>().map(|s| s.map(|v| v as f32 / scale));
            fill(path, &mut samples, scaled)?;
        }
    }
    Ok(Decoded {
        rate: spec.sample_rate,
        channels: spec.channels,
        samples,
    })
}

/// The sample widths this decoder reads, and how it carries each.
enum Width {
    Float,
    Int(u16),
}

/// Refuses a header whose samples this decoder does not read.
fn check_width(path: &Path, spec: hound::WavSpec) -> Result<Width, DecodeError> {
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => Ok(Width::Float),
        (SampleFormat::Int, bits @ 1..=32) => Ok(Width::Int(bits)),
        (format, bits) => Err(malformed(
            path,
            format!("{bits}-bit {format:?} samples are not supported"),
        )),
    }
}

/// Drains a sample iterator into `block`, attributing any failure to
/// `path`. The iterator yields the count the header gave, which is what
/// the block was sized from, so every sample fits.
fn fill(
    path: &Path,
    block: &mut Block<Pcm>,
    samples: impl Iterator<Item = hound::Result<f32>>,
) -> Result<(), DecodeError> {
    for sample in samples {
        let pushed = block.push(sample.map_err(|e| hound_error(path, e))?);
        debug_assert!(
            pushed,
            "the header counted fewer samples than the file holds"
        );
    }
    Ok(())
}

fn malformed(path: &Path, reason: String) -> DecodeError {
    DecodeError::Malformed {
        path: path.into(),
        reason,
    }
}

fn hound_error(path: &Path, e: hound::Error) -> DecodeError {
    match e {
        hound::Error::IoError(source) => DecodeError::Io {
            path: path.into(),
            source,
        },
        other => malformed(path, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use hound::{WavSpec, WavWriter};

    use super::*;

    /// Decodes against a ledger that refuses nothing.
    fn decode(path: &Path) -> Result<Decoded, DecodeError> {
        super::decode(path, &Ledger::new(usize::MAX))
    }

    fn spec(channels: u16, rate: u32, bits: u16, format: SampleFormat) -> WavSpec {
        WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: format,
        }
    }

    fn write<S: hound::Sample + Copy>(path: &Path, spec: WavSpec, samples: &[S]) {
        let mut w = WavWriter::create(path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn the_block_is_exactly_the_file_and_is_refused_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ten.wav");
        write(&path, spec(1, 48_000, 16, SampleFormat::Int), &[0i16; 10]);
        let bytes = 10 * size_of::<Pcm>();

        let ledger = Ledger::new(bytes);
        let d = super::decode(&path, &ledger).unwrap();
        assert_eq!(d.samples.capacity(), 10);
        assert_eq!(ledger.in_use(), bytes);
        drop(d);
        assert_eq!(ledger.in_use(), 0);

        let ledger = Ledger::new(bytes - 1);
        assert!(matches!(
            super::decode(&path, &ledger).unwrap_err(),
            DecodeError::OverBudget {
                source: OverBudget { requested, .. },
                ..
            } if requested == bytes
        ));
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn sixteen_bit_stereo_lands_on_the_unit_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(
            &path,
            spec(2, 44_100, 16, SampleFormat::Int),
            &[0i16, 16_384, -32_768, 32_767],
        );
        let d = decode(&path).unwrap();
        assert_eq!((d.rate, d.channels), (44_100, 2));
        assert_eq!(&d.samples[..], &[0.0, 0.5, -1.0, 32_767.0 / 32_768.0]);
    }

    #[test]
    fn every_integer_width_lands_on_the_same_range() {
        let dir = tempfile::tempdir().unwrap();
        let p8 = dir.path().join("8.wav");
        write(&p8, spec(1, 48_000, 8, SampleFormat::Int), &[64i8, -128]);
        assert_eq!(&decode(&p8).unwrap().samples[..], &[0.5, -1.0]);

        let p24 = dir.path().join("24.wav");
        write(
            &p24,
            spec(1, 48_000, 24, SampleFormat::Int),
            &[1i32 << 22, -(1 << 23)],
        );
        assert_eq!(&decode(&p24).unwrap().samples[..], &[0.5, -1.0]);

        let p32 = dir.path().join("32.wav");
        write(
            &p32,
            spec(1, 48_000, 32, SampleFormat::Int),
            &[1i32 << 30, i32::MIN],
        );
        assert_eq!(&decode(&p32).unwrap().samples[..], &[0.5, -1.0]);
    }

    #[test]
    fn float_is_taken_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.wav");
        write(
            &path,
            spec(1, 96_000, 32, SampleFormat::Float),
            &[0.25f32, -0.75, 1.5],
        );
        let d = decode(&path).unwrap();
        assert_eq!(d.rate, 96_000);
        assert_eq!(&d.samples[..], &[0.25, -0.75, 1.5]);
    }

    #[test]
    fn a_file_that_is_not_a_wav_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noise.bin");
        std::fs::write(&path, b"hello").unwrap();
        assert!(matches!(
            decode(&path).unwrap_err(),
            DecodeError::Malformed { path: p, .. } if *p == *path
        ));
    }

    #[test]
    fn a_truncated_file_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.wav");
        std::fs::write(&path, b"RIFF\x24\0\0\0WAVEfmt ").unwrap();
        assert!(matches!(
            decode(&path).unwrap_err(),
            DecodeError::Malformed { .. } | DecodeError::Io { .. }
        ));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.wav");
        assert!(matches!(
            decode(&path).unwrap_err(),
            DecodeError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound
        ));
    }
}
