//! Conversion of decoded audio into the project format.
//!
//! A decoder, whatever file format it reads, produces one plain value: a
//! [`Decoded`], interleaved `f32` at whatever rate and channel count the
//! file had. [`convert`] takes that value to an [`AudioBuffer`] at the
//! project's rate. Decoders know formats; this module knows the project
//! format; nothing knows both.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, ResampleError, Resampler, ResamplerConstructionError};
use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError, check_channels};
use crate::config::SAMPLE_RATE;

/// Frames the resampler works on per pass. Only affects speed and memory,
/// not the result: the whole clip goes through in one call.
const RESAMPLE_CHUNK: usize = 1024;

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
#[derive(Debug, Error)]
pub enum ConvertError {
    /// The source rate was zero.
    #[error("sample rate of 0 Hz")]
    ZeroRate,
    /// The resampler could not be built for this rate pair.
    #[error("cannot resample from {rate} Hz to {SAMPLE_RATE} Hz: {source}")]
    Rate {
        rate: u32,
        source: ResamplerConstructionError,
    },
    /// The resampler failed on the audio itself.
    #[error(transparent)]
    Resample(#[from] ResampleError),
    /// The channel count or frame alignment does not fit a buffer.
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// Converts decoded audio to the project format.
///
/// Audio at another rate is resampled to [`SAMPLE_RATE`]; audio already
/// there is only quantised. Channel count is kept as is: mono stays mono,
/// stereo stays stereo. More channels than a buffer holds is an error, not
/// a mixdown.
pub fn convert(decoded: &Decoded) -> Result<AudioBuffer, ConvertError> {
    if decoded.rate == 0 {
        return Err(ConvertError::ZeroRate);
    }
    if decoded.rate == SAMPLE_RATE {
        return Ok(AudioBuffer::from_f32(decoded.channels, &decoded.samples)?);
    }
    // Check the shape before resampling so the error names the real
    // problem rather than a buffer-size mismatch inside the resampler.
    check_channels(decoded.channels)?;
    let channels = usize::from(decoded.channels);
    if !decoded.samples.len().is_multiple_of(channels) {
        return Err(AudioError::RaggedFrames {
            samples: decoded.samples.len(),
            channels: decoded.channels,
        }
        .into());
    }
    let frames = decoded.samples.len() / channels;
    let resampled = resample(&decoded.samples, decoded.rate, channels, frames)?;
    Ok(AudioBuffer::from_f32(decoded.channels, &resampled)?)
}

/// Resamples a whole interleaved clip from `rate` to [`SAMPLE_RATE`].
///
/// Synchronous (the ratio is exact for the rate pair), anti-aliased, and
/// run over the clip in one call so the resampler's own delay is trimmed:
/// the result is as many frames as the clip would have had at the target
/// rate, give or take rounding, and starts where the clip starts.
fn resample(
    samples: &[f32],
    rate: u32,
    channels: usize,
    frames: usize,
) -> Result<Vec<f32>, ConvertError> {
    let mut resampler = Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        channels,
        FixedSync::Input,
    )
    .map_err(|source| ConvertError::Rate { rate, source })?;
    let input = InterleavedSlice::new(samples, channels, frames)
        .expect("frame count was derived from the slice length");
    let output = resampler.process_all(&input, frames, None)?;
    Ok(output.take_data())
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

    /// A sine of `hz` lasting `seconds` at `rate`, mono.
    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    /// Zero crossings going upward: twice this per second is the frequency.
    fn rising_crossings(s: &[f32]) -> usize {
        s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn other_rate_is_resampled_to_the_project_rate() {
        let d = at(44_100, 1, sine(44_100, 440.0, 1.0));
        let b = convert(&d).unwrap();
        assert_eq!(b.channels(), 1);
        // One second in, one second out, within a frame or two of rounding.
        assert!(
            (b.frames() as i64 - SAMPLE_RATE as i64).abs() <= 2,
            "{}",
            b.frames()
        );
        // Same pitch: 440 rising crossings per second (edges may lose one).
        let out = b.to_f32();
        assert!((rising_crossings(&out) as i64 - 440).abs() <= 2);
        // Same level: a pure tone's peak survives resampling.
        let peak = out.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.02, "{peak}");
    }

    #[test]
    fn resampling_keeps_channels_separate() {
        let left = sine(96_000, 100.0, 0.5);
        let right = vec![0.0; left.len()];
        let interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        let b = convert(&at(96_000, 2, interleaved)).unwrap();
        assert_eq!(b.channels(), 2);
        assert!((b.frames() as i64 - 24_000).abs() <= 2, "{}", b.frames());
        let out = b.to_f32();
        let right_peak = out
            .iter()
            .skip(1)
            .step_by(2)
            .fold(0.0f32, |m, &x| m.max(x.abs()));
        let left_peak = out.iter().step_by(2).fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(right_peak < 0.001, "{right_peak}");
        assert!((left_peak - 0.5).abs() < 0.02, "{left_peak}");
    }

    #[test]
    fn resampling_an_empty_clip_is_empty() {
        let b = convert(&at(44_100, 2, vec![])).unwrap();
        assert_eq!(b.frames(), 0);
        assert_eq!(b.channels(), 2);
    }

    #[test]
    fn ragged_frames_are_refused_before_resampling() {
        assert!(matches!(
            convert(&at(44_100, 2, vec![0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
        assert!(matches!(
            convert(&at(44_100, 6, vec![0.0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        ));
    }

    #[test]
    fn zero_rate_is_refused() {
        assert!(matches!(
            convert(&at(0, 1, vec![])).unwrap_err(),
            ConvertError::ZeroRate
        ));
    }

    #[test]
    fn too_many_channels_is_refused_not_mixed() {
        assert!(matches!(
            convert(&at(SAMPLE_RATE, 6, vec![0.0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        ));
    }

    #[test]
    fn ragged_frames_are_refused() {
        assert!(matches!(
            convert(&at(SAMPLE_RATE, 2, vec![0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
    }
}
