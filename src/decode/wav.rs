//! WAV: RIFF/WAVE with integer PCM at 8 to 32 bits, or 32-bit float.
//!
//! Integer samples are reported at their own width, float samples as they
//! are. Anything else `hound` can open but we do not expect is reported as
//! malformed rather than guessed at.

use std::path::Path;

use hound::{SampleFormat, WavReader};

use super::{DecodeError, Decoded, Samples};
use crate::ledger::Reservation;

/// What [`decode`] will allocate for this file, from the header alone:
/// every width is carried in a four-byte type, `i32` or `f32`. A file `decode`
/// would refuse is refused here, with the same error, so a bad file is
/// named as bad rather than as too big.
pub fn byte_cost(path: &Path) -> Result<usize, DecodeError> {
    let reader = WavReader::open(path).map_err(|e| hound_error(path, e))?;
    check_width(path, reader.spec())?;
    bytes(path, &reader)
}

/// Bytes the samples of an open reader will take.
fn bytes<R: std::io::Read>(path: &Path, reader: &WavReader<R>) -> Result<usize, DecodeError> {
    (reader.len() as usize).checked_mul(4).ok_or_else(|| {
        malformed(
            path,
            format!("{} samples do not fit in memory", reader.len()),
        )
    })
}

pub fn decode(path: &Path, reservation: &Reservation) -> Result<Decoded, DecodeError> {
    let mut reader = WavReader::open(path).map_err(|e| hound_error(path, e))?;
    let spec = reader.spec();
    let width = check_width(path, spec)?;
    reservation
        .can_hold(bytes(path, &reader)?)
        .map_err(|source| DecodeError::ShortReservation {
            path: path.into(),
            source,
        })?;
    let len = reader.len() as usize;
    let samples = match width {
        Width::Float => Samples::Float(collect(path, len, reader.samples::<f32>())?),
        Width::Int(bits) => Samples::Int {
            bits,
            samples: collect(path, len, reader.samples::<i32>())?,
        },
    };
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

/// Drains a sample iterator into a buffer of exactly `len` capacity,
/// attributing any failure to `path`.
///
/// Not `collect`: collecting through a `Result` loses the exact size hint
/// and the buffer grows by doubling, so it would end up with as much as
/// twice the capacity [`byte_cost`] reserved for it.
fn collect<T>(
    path: &Path,
    len: usize,
    samples: impl Iterator<Item = hound::Result<T>>,
) -> Result<Vec<T>, DecodeError> {
    let mut out = Vec::with_capacity(len);
    for sample in samples {
        out.push(sample.map_err(|e| hound_error(path, e))?);
    }
    Ok(out)
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
        other => DecodeError::Malformed {
            path: path.into(),
            reason: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use hound::{WavSpec, WavWriter};

    use super::*;
    use crate::ledger::{Ledger, ShortReservation};
    use crate::testing::paid;

    /// Decodes with a reservation that pays for anything.
    fn decode(path: &Path) -> Result<Decoded, DecodeError> {
        super::decode(path, &paid())
    }

    #[test]
    fn a_short_reservation_is_refused_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        write(&path, spec(1, 48_000, 16, SampleFormat::Int), &[0i16; 10]);
        assert_eq!(byte_cost(&path).unwrap(), 40);
        let ledger = Ledger::new(100);
        let enough = ledger.reserve(40).unwrap();
        assert!(super::decode(&path, &enough).is_ok());
        let short = ledger.reserve(39).unwrap();
        assert!(matches!(
            super::decode(&path, &short).unwrap_err(),
            DecodeError::ShortReservation {
                source: ShortReservation {
                    needed: 40,
                    reserved: 39
                },
                ..
            }
        ));
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

    fn int(s: &Samples) -> (u16, &[i32]) {
        match s {
            Samples::Int { bits, samples } => (*bits, samples),
            Samples::Float(_) => panic!("expected integer samples"),
        }
    }

    #[test]
    fn sixteen_bit_stereo_is_reported_as_16_bit_integers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(
            &path,
            spec(2, 44_100, 16, SampleFormat::Int),
            &[0i16, 16_384, -32_768, 32_767],
        );
        let d = decode(&path).unwrap();
        assert_eq!((d.rate, d.channels), (44_100, 2));
        assert_eq!(int(&d.samples), (16, &[0, 16_384, -32_768, 32_767][..]));
    }

    #[test]
    fn every_integer_width_is_reported_at_its_own_width() {
        let dir = tempfile::tempdir().unwrap();
        let p8 = dir.path().join("8.wav");
        write(&p8, spec(1, 48_000, 8, SampleFormat::Int), &[64i8, -128]);
        assert_eq!(int(&decode(&p8).unwrap().samples), (8, &[64, -128][..]));

        let p24 = dir.path().join("24.wav");
        write(
            &p24,
            spec(1, 48_000, 24, SampleFormat::Int),
            &[1i32 << 22, -(1 << 23)],
        );
        assert_eq!(
            int(&decode(&p24).unwrap().samples),
            (24, &[1 << 22, -(1 << 23)][..])
        );

        let p32 = dir.path().join("32.wav");
        write(
            &p32,
            spec(1, 48_000, 32, SampleFormat::Int),
            &[1i32 << 30, i32::MIN],
        );
        assert_eq!(
            int(&decode(&p32).unwrap().samples),
            (32, &[1 << 30, i32::MIN][..])
        );
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
        assert_eq!(d.samples, Samples::Float(vec![0.25, -0.75, 1.5]));
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
}
