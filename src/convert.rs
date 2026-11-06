//! Conversion of decoded audio into the project format.
//!
//! A decoder, whatever file format it reads, produces one plain value: a
//! [`Decoded`], interleaved `f32` at whatever rate and channel count the
//! file had. [`convert`] takes that value to an [`AudioBuffer`] at the
//! project's rate. Decoders know formats; this module knows the project
//! format; nothing knows both.

use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError};
use crate::config::SAMPLE_RATE;

/// Audio as a decoder found it: not yet in the project format.
///
/// Samples are interleaved and normalised to `-1.0..=1.0`, with no promise
/// that they stay in range; conversion clamps.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// Sample rate of the source, in Hz.
    pub rate: u32,
    /// Interleaved channel count of the source.
    pub channels: u16,
    /// Interleaved samples, `channels` per frame.
    pub samples: Vec<f32>,
}

/// Reasons decoded audio cannot become project audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConvertError {
    /// The source rate was zero.
    #[error("sample rate of 0 Hz")]
    ZeroRate,
    /// The source rate differs from [`SAMPLE_RATE`] and cannot be resampled.
    #[error("cannot resample from {0} Hz to {SAMPLE_RATE} Hz")]
    Unsupported(u32),
    /// The channel count or frame alignment does not fit a buffer.
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// Converts decoded audio to the project format.
///
/// Channel count is kept as is: mono stays mono, stereo stays stereo. More
/// channels than a buffer holds is an error, not a mixdown.
pub fn convert(decoded: &Decoded) -> Result<AudioBuffer, ConvertError> {
    if decoded.rate == 0 {
        return Err(ConvertError::ZeroRate);
    }
    if decoded.rate != SAMPLE_RATE {
        return Err(ConvertError::Unsupported(decoded.rate));
    }
    Ok(AudioBuffer::from_f32(decoded.channels, &decoded.samples)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rate: u32, channels: u16, samples: Vec<f32>) -> Decoded {
        Decoded {
            rate,
            channels,
            samples,
        }
    }

    #[test]
    fn same_rate_is_quantised_and_keeps_its_channels() {
        let d = at(SAMPLE_RATE, 2, vec![0.0, 0.5, -0.5, 1.0]);
        let b = convert(&d).unwrap();
        assert_eq!(b.channels(), 2);
        assert_eq!(b.frames(), 2);
        let back = b.to_f32();
        for (want, got) in d.samples.iter().zip(&back) {
            assert!(
                (want.clamp(-1.0, 1.0) - got).abs() < 0.001,
                "{want} vs {got}"
            );
        }
    }

    #[test]
    fn zero_rate_is_refused() {
        assert_eq!(
            convert(&at(0, 1, vec![])).unwrap_err(),
            ConvertError::ZeroRate
        );
    }

    #[test]
    fn too_many_channels_is_refused_not_mixed() {
        assert_eq!(
            convert(&at(SAMPLE_RATE, 6, vec![0.0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        );
    }

    #[test]
    fn ragged_frames_are_refused() {
        assert_eq!(
            convert(&at(SAMPLE_RATE, 2, vec![0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        );
    }
}
