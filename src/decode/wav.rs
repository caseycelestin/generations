//! WAV: RIFF/WAVE with integer PCM at 8 to 32 bits, or 32-bit float.
//!
//! Integer samples are scaled by their own bit depth so every width lands
//! on the same `-1.0..1.0`; float samples are taken as they are. Anything
//! else `hound` can open but we do not expect is reported as malformed
//! rather than guessed at.

use std::path::Path;

use hound::{SampleFormat, WavReader};

use super::{DecodeError, Decoded};

pub fn decode(path: &Path) -> Result<Decoded, DecodeError> {
    let malformed = |reason: String| DecodeError::Malformed {
        path: path.into(),
        reason,
    };
    let mut reader = WavReader::open(path).map_err(|e| hound_error(path, e))?;
    let spec = reader.spec();
    let samples: Result<Vec<f32>, hound::Error> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => reader.samples::<f32>().collect(),
        (SampleFormat::Int, bits @ 1..=32) => {
            let scale = (1u64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect()
        }
        (format, bits) => {
            return Err(malformed(format!(
                "{bits}-bit {format:?} samples are not supported"
            )));
        }
    };
    Ok(Decoded {
        rate: spec.sample_rate,
        channels: spec.channels,
        samples: samples.map_err(|e| hound_error(path, e))?,
    })
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

    fn close(a: &[f32], b: &[f32], tol: f32) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() <= tol, "{x} vs {y}");
        }
    }

    #[test]
    fn sixteen_bit_stereo_keeps_rate_channels_and_scale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(
            &path,
            spec(2, 44_100, 16, SampleFormat::Int),
            &[0i16, 16_384, -32_768, 32_767],
        );
        let d = decode(&path).unwrap();
        assert_eq!((d.rate, d.channels), (44_100, 2));
        close(&d.samples, &[0.0, 0.5, -1.0, 32_767.0 / 32_768.0], 0.0);
    }

    #[test]
    fn every_integer_width_lands_on_the_same_scale() {
        let dir = tempfile::tempdir().unwrap();
        let p8 = dir.path().join("8.wav");
        write(&p8, spec(1, 48_000, 8, SampleFormat::Int), &[64i8, -128]);
        close(&decode(&p8).unwrap().samples, &[0.5, -1.0], 0.0);

        let p24 = dir.path().join("24.wav");
        write(
            &p24,
            spec(1, 48_000, 24, SampleFormat::Int),
            &[1i32 << 22, -(1 << 23)],
        );
        close(&decode(&p24).unwrap().samples, &[0.5, -1.0], 0.0);

        let p32 = dir.path().join("32.wav");
        write(
            &p32,
            spec(1, 48_000, 32, SampleFormat::Int),
            &[1i32 << 30, i32::MIN],
        );
        close(&decode(&p32).unwrap().samples, &[0.5, -1.0], 0.0);
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
        close(&d.samples, &[0.25, -0.75, 1.5], 0.0);
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
