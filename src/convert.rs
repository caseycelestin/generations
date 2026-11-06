//! Conversion of decoded audio into the project format.
//!
//! A decoder, whatever file format it reads, produces one plain value: a
//! [`Decoded`], interleaved samples at whatever rate, channel count and
//! width the file had. [`convert`] takes that value to an [`AudioBuffer`]
//! at the project's rate, and is the only place that decides whether
//! anything needs doing: a file already at the project's rate and width
//! is stored byte for byte. Decoders know formats; this module knows the
//! project format; nothing knows both.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, ResampleError, Resampler, ResamplerConstructionError};
use thiserror::Error;

use crate::audio::{AudioBuffer, AudioError, check_channels};
use crate::config::{Pcm, SAMPLE_RATE};
use crate::ledger::{Reservation, ShortReservation};

/// Frames the resampler works on per pass. Only affects speed and memory,
/// not the result: the whole clip goes through in one call.
const RESAMPLE_CHUNK: usize = 1024;

/// Audio as a decoder found it, in whatever rate and width the file had.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// Sample rate of the source, in Hz.
    pub rate: u32,
    /// Interleaved channel count of the source.
    pub channels: u16,
    /// Interleaved samples, `channels` per frame.
    pub samples: Samples,
}

/// The samples of a [`Decoded`], as the file stored them.
///
/// A decoder reports what it read and nothing more: integers at their own
/// width, floats as floats. Whether that happens to be the project's
/// width is for [`convert`] to notice.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    /// Signed integers of `bits` bits, each in `-2^(bits-1)..2^(bits-1)`,
    /// carried in the smallest common type.
    Int { bits: u16, samples: Vec<i32> },
    /// Floats normalised to `-1.0..=1.0`, with no promise of staying in
    /// range; conversion clamps.
    Float(Vec<f32>),
}

impl Samples {
    fn len(&self) -> usize {
        match self {
            Samples::Int { samples, .. } => samples.len(),
            Samples::Float(v) => v.len(),
        }
    }

    /// Normalises to `f32`, scaling integers by their own width so every
    /// width lands on the same range.
    fn into_f32(self) -> Vec<f32> {
        match self {
            Samples::Int { bits, samples } => {
                let scale = (1u64 << (bits - 1)) as f32;
                samples.into_iter().map(|v| v as f32 / scale).collect()
            }
            Samples::Float(v) => v,
        }
    }
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
    /// The reservation offered does not pay for what the conversion
    /// allocates. Nothing was allocated.
    #[error("converting {0}")]
    ShortReservation(#[from] ShortReservation),
    /// The audio is longer than this machine can count bytes for.
    #[error("{samples} samples at {rate} Hz are too large to convert")]
    TooLarge { samples: usize, rate: u32 },
    /// The resampler failed on the audio itself.
    #[error(transparent)]
    Resample(#[from] ResampleError),
    /// The channel count or frame alignment does not fit a buffer.
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// What [`convert`] will allocate for this audio, in bytes, over and
/// above the audio itself: its working copies and the result. Reserved
/// before it runs. Audio `convert` would refuse is refused here with the
/// same error, so a bad file is named as bad rather than as too big.
///
/// Follows the same branches as `convert`: the exact path allocates only
/// the PCM result; the same-rate path an `f32` copy and the result; the
/// resampling path an `f32` copy, the resampled `f32` output and the
/// result. The resampled output is as large as the resampler says it
/// will make it, which is the clip at the new rate plus its own delay and
/// one chunk, since it allocates that and truncates without shrinking.
/// It is an upper bound: the `f32` copy is charged even though the
/// standard library can turn the integer buffer into it in place and
/// moves a float buffer as it is. The ledger would rather over-count than
/// rest on an optimisation. The resampler's own state is a chunk, not a
/// clip, and is not counted.
pub fn byte_cost(decoded: &Decoded) -> Result<usize, ConvertError> {
    let (channels, frames) = check_shape(decoded)?;
    cost(decoded, channels, frames)
}

/// [`byte_cost`] for audio whose shape has been checked.
fn cost(decoded: &Decoded, channels: usize, frames: usize) -> Result<usize, ConvertError> {
    let samples = decoded.samples.len();
    let too_large = || ConvertError::TooLarge {
        samples,
        rate: decoded.rate,
    };
    let bytes = |n: usize, size: usize| n.checked_mul(size).ok_or_else(too_large);
    let add = |a: usize, b: usize| a.checked_add(b).ok_or_else(too_large);

    let exact =
        matches!(&decoded.samples, Samples::Int { bits, .. } if u32::from(*bits) == Pcm::BITS);
    if decoded.rate == SAMPLE_RATE {
        let pcm_out = bytes(samples, size_of::<Pcm>())?;
        return if exact {
            Ok(pcm_out)
        } else {
            add(bytes(samples, size_of::<f32>())?, pcm_out)
        };
    }
    let out_frames = resampler(decoded.rate, channels)?.process_all_needed_output_len(frames);
    let out_samples = out_frames.checked_mul(channels).ok_or_else(too_large)?;
    let f32_in = bytes(samples, size_of::<f32>())?;
    let f32_out = bytes(out_samples, size_of::<f32>())?;
    let pcm_out = bytes(out_samples, size_of::<Pcm>())?;
    add(add(f32_in, f32_out)?, pcm_out)
}

/// Refuses audio no [`AudioBuffer`] could hold or no resampler could take,
/// and returns its channel count and frame count. Checked first so the
/// error names the real problem rather than a buffer-size mismatch inside
/// the resampler.
fn check_shape(decoded: &Decoded) -> Result<(usize, usize), ConvertError> {
    if decoded.rate == 0 {
        return Err(ConvertError::ZeroRate);
    }
    check_channels(decoded.channels)?;
    let channels = usize::from(decoded.channels);
    if !decoded.samples.len().is_multiple_of(channels) {
        return Err(AudioError::RaggedFrames {
            samples: decoded.samples.len(),
            channels: decoded.channels,
        }
        .into());
    }
    Ok((channels, decoded.samples.len() / channels))
}

/// Converts decoded audio to the project format.
///
/// Audio at another rate is resampled to [`SAMPLE_RATE`]; audio already
/// there is only quantised. Channel count is kept as is: mono stays mono,
/// stereo stays stereo. More channels than a buffer holds is an error, not
/// a mixdown.
///
/// `reservation` must cover [`byte_cost`] of the audio; a short one is
/// refused before anything is allocated. The caller keeps it for as long
/// as the result lives.
pub fn convert(decoded: Decoded, reservation: &Reservation) -> Result<AudioBuffer, ConvertError> {
    let (channels, frames) = check_shape(&decoded)?;
    reservation.claim(cost(&decoded, channels, frames)?)?;

    if decoded.rate == SAMPLE_RATE {
        if let Samples::Int { bits, samples } = &decoded.samples
            && u32::from(*bits) == Pcm::BITS
        {
            // Already the project format: nothing to do but store it.
            let pcm: Vec<Pcm> = samples.iter().map(|&v| v as Pcm).collect();
            return Ok(AudioBuffer::new(decoded.channels, pcm)?);
        }
        return Ok(AudioBuffer::from_f32(
            decoded.channels,
            &decoded.samples.into_f32(),
        )?);
    }
    // Resampling is float work whatever the file held.
    let float = decoded.samples.into_f32();
    let resampled = resample(&float, decoded.rate, channels, frames)?;
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
    let mut resampler = resampler(rate, channels)?;
    let input = InterleavedSlice::new(samples, channels, frames)
        .expect("frame count was derived from the slice length");
    let output = resampler.process_all(&input, frames, None)?;
    Ok(output.take_data())
}

/// A resampler from `rate` to [`SAMPLE_RATE`] for `channels` channels.
fn resampler(rate: u32, channels: usize) -> Result<Fft<f32>, ConvertError> {
    Fft::<f32>::new(
        rate as usize,
        SAMPLE_RATE as usize,
        RESAMPLE_CHUNK,
        channels,
        FixedSync::Input,
    )
    .map_err(|source| ConvertError::Rate { rate, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use crate::testing::paid;

    /// Converts with a reservation that pays for anything.
    fn convert(decoded: Decoded) -> Result<AudioBuffer, ConvertError> {
        super::convert(decoded, &paid())
    }

    #[test]
    fn a_short_reservation_is_refused_before_converting() {
        let d = int(SAMPLE_RATE, 1, 24, vec![0; 10]);
        let needed = byte_cost(&d).unwrap();
        let ledger = Ledger::new(usize::MAX);
        assert!(super::convert(d.clone(), &ledger.reserve(needed).unwrap()).is_ok());
        assert!(matches!(
            super::convert(d, &ledger.reserve(needed - 1).unwrap()).unwrap_err(),
            ConvertError::ShortReservation(ShortReservation { needed: n, reserved: r })
                if n == needed && r == needed - 1
        ));
    }

    fn at(rate: u32, channels: u16, samples: Vec<f32>) -> Decoded {
        Decoded {
            rate,
            channels,
            samples: Samples::Float(samples),
        }
    }

    fn int(rate: u32, channels: u16, bits: u16, samples: Vec<i32>) -> Decoded {
        Decoded {
            rate,
            channels,
            samples: Samples::Int { bits, samples },
        }
    }

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
    }

    #[test]
    fn byte_cost_follows_the_path_convert_takes() {
        let pcm = size_of::<Pcm>();
        // Exact: only the result.
        assert_eq!(
            byte_cost(&int(SAMPLE_RATE, 1, 16, vec![0; 10])).unwrap(),
            10 * pcm
        );
        // Same rate, other width or float: an f32 copy and the result.
        assert_eq!(
            byte_cost(&int(SAMPLE_RATE, 1, 24, vec![0; 10])).unwrap(),
            10 * 4 + 10 * pcm
        );
        assert_eq!(
            byte_cost(&at(SAMPLE_RATE, 2, vec![0.0; 10])).unwrap(),
            10 * 4 + 10 * pcm
        );
        // Other rate: f32 in, f32 out as large as the resampler will make
        // it (at least the clip at the new rate), and the result.
        let cost = byte_cost(&at(44_100, 2, vec![0.0; 10])).unwrap();
        let out = resampler(44_100, 2)
            .unwrap()
            .process_all_needed_output_len(5)
            * 2;
        assert!(out >= (5u64 * u64::from(SAMPLE_RATE)).div_ceil(44_100) as usize * 2);
        assert_eq!(cost, 10 * 4 + out * 4 + out * pcm);
        // The buffer convert makes really does fit the charge.
        let b = convert(at(44_100, 2, vec![0.0; 10])).unwrap();
        assert!(b.bytes() <= out * pcm);
    }

    #[test]
    fn byte_cost_refuses_what_convert_refuses() {
        assert!(matches!(
            byte_cost(&at(0, 1, vec![0.0; 10])),
            Err(ConvertError::ZeroRate)
        ));
        assert!(matches!(
            byte_cost(&at(44_100, 2, vec![0.0; 3])),
            Err(ConvertError::Audio(AudioError::RaggedFrames { .. }))
        ));
        assert!(matches!(
            byte_cost(&at(44_100, 6, vec![0.0; 6])),
            Err(ConvertError::Audio(AudioError::BadChannelCount(6)))
        ));
    }

    #[test]
    fn project_width_at_the_project_rate_is_stored_byte_for_byte() {
        let want = [0, 1, -1, 2001, i32::from(Pcm::MAX), i32::from(Pcm::MIN)];
        let b = convert(int(SAMPLE_RATE, 2, 16, want.to_vec())).unwrap();
        let got: Vec<i32> = b.samples().iter().map(|&s| i32::from(s)).collect();
        assert_eq!(got, want);
        assert_eq!(b.channels(), 2);
    }

    #[test]
    fn other_widths_at_the_project_rate_are_quantised() {
        // 24-bit half scale and 8-bit half scale both land on 16-bit half scale.
        let b24 = convert(int(SAMPLE_RATE, 1, 24, vec![1 << 22, -(1 << 23)])).unwrap();
        let b8 = convert(int(SAMPLE_RATE, 1, 8, vec![64, -128])).unwrap();
        for b in [b24, b8] {
            let s = b.samples();
            assert!((i32::from(s[0]) - 16_384).abs() <= 1, "{}", s[0]);
            assert!(s[1] <= Pcm::MIN + 1, "{}", s[1]);
        }
    }

    #[test]
    fn integers_at_another_rate_are_resampled_like_float() {
        let tone: Vec<i32> = sine(44_100, 440.0, 1.0)
            .iter()
            .map(|s| (s * 8_388_608.0) as i32)
            .collect();
        let b = convert(int(44_100, 1, 24, tone)).unwrap();
        assert!((b.frames() as i64 - SAMPLE_RATE as i64).abs() <= 2);
        let out = b.to_f32();
        assert!((rising_crossings(&out) as i64 - 440).abs() <= 2);
        // Level survives: the width scaling in the resample path is right.
        assert!((peak(&out) - 0.5).abs() < 0.02, "{}", peak(&out));
    }

    #[test]
    fn integer_shape_errors_are_the_same_as_float() {
        assert!(matches!(
            convert(int(SAMPLE_RATE, 2, 16, vec![0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
        assert!(matches!(
            convert(int(44_100, 6, 16, vec![0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        ));
    }

    #[test]
    fn same_rate_is_quantised_and_keeps_its_channels() {
        let samples = vec![0.0, 0.5, -0.5, 1.0];
        let b = convert(at(SAMPLE_RATE, 2, samples.clone())).unwrap();
        assert_eq!(b.channels(), 2);
        assert_eq!(b.frames(), 2);
        let back = b.to_f32();
        for (want, got) in samples.iter().zip(&back) {
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
        let b = convert(d).unwrap();
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
        assert!((peak(&out) - 0.5).abs() < 0.02, "{}", peak(&out));
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
        let b = convert(at(96_000, 2, interleaved)).unwrap();
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
        let b = convert(at(44_100, 2, vec![])).unwrap();
        assert_eq!(b.frames(), 0);
        assert_eq!(b.channels(), 2);
    }

    #[test]
    fn ragged_frames_are_refused_before_resampling() {
        assert!(matches!(
            convert(at(44_100, 2, vec![0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
        assert!(matches!(
            convert(at(44_100, 6, vec![0.0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        ));
    }

    #[test]
    fn zero_rate_is_refused() {
        assert!(matches!(
            convert(at(0, 1, vec![])).unwrap_err(),
            ConvertError::ZeroRate
        ));
    }

    #[test]
    fn too_many_channels_is_refused_not_mixed() {
        assert!(matches!(
            convert(at(SAMPLE_RATE, 6, vec![0.0; 6])).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(6))
        ));
    }

    #[test]
    fn ragged_frames_are_refused() {
        assert!(matches!(
            convert(at(SAMPLE_RATE, 2, vec![0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
    }
}
