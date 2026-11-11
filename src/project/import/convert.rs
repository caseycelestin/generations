//! Conversion of decoded audio into the engine format.
//!
//! A decoder produces a [`DecodedAudio`]: the engine's sample type at the
//! file's own rate. [`convert`] takes that to an [`AudioBuffer`] at the
//! engine's rate, and is the only place that decides whether anything
//! needs doing. A clip already at the rate is wrapped as it is, in the
//! block the decoder paid for, with nothing copied. A clip at another
//! rate is resampled into a fresh block paid for here, and the decoded
//! block is freed on the way out.

use std::iter::repeat_n;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, ResampleError, Resampler, ResamplerConstructionError};
use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::audio::{AudioError, check_channels};
use crate::config::{Pcm, SAMPLE_RATE};
use crate::memory::{Block, Ledger, OverBudget};
use crate::project::import::decode::DecodedAudio;

/// Frames the resampler works on per pass. Only affects speed and memory,
/// not the result: the whole clip goes through in one call.
const RESAMPLE_CHUNK: usize = 1024;

/// Reasons decoded audio cannot become engine audio.
#[derive(Debug, Error)]
pub enum ConvertError {
    /// The resampler could not be built for this rate pair.
    #[error("cannot resample from {rate} Hz to {SAMPLE_RATE} Hz: {source}")]
    Rate {
        rate: u32,
        source: ResamplerConstructionError,
    },
    /// The resampled clip would put audio memory over its budget.
    /// Nothing was allocated.
    #[error(transparent)]
    OverBudget(#[from] OverBudget),
    /// The resampler failed on the audio itself.
    #[error(transparent)]
    Resample(#[from] ResampleError),
    /// The channel count or frame alignment does not fit a buffer.
    #[error(transparent)]
    Audio(#[from] AudioError),
}

/// Converts decoded audio to the engine format.
///
/// Audio at another rate is resampled to [`SAMPLE_RATE`] into a block
/// allocated from `ledger`; audio already there is wrapped in the block
/// it arrived in. Channel count is kept as is: mono stays mono, stereo
/// stays stereo. More channels than a buffer holds is an error, not a
/// mixdown, and is refused before anything is paid for.
///
/// Resampling holds both blocks at once, the decoded clip and the result,
/// so the budget must cover the sum for the length of the call. The
/// result's block is sized by the resampler's worst case, the clip at the
/// new rate plus its own delay and one chunk, and is truncated to what
/// came out; the capacity stays paid for while the buffer lives.
pub fn convert(decoded: DecodedAudio, ledger: &Ledger) -> Result<AudioBuffer, ConvertError> {
    let (channels, frames) = check_shape(&decoded)?;
    if decoded.rate == SAMPLE_RATE {
        return AudioBuffer::new(decoded.channels, decoded.samples).map_err(|(e, _)| e.into());
    }
    let mut resampler = resampler(decoded.rate, channels)?;
    let out_frames = resampler.process_all_needed_output_len(frames);
    // Saturates rather than refuses: the ledger turns a count it cannot
    // put in bytes into its own refusal.
    let out_samples = out_frames.saturating_mul(channels);
    let mut out: Block<Pcm> = ledger.alloc(out_samples)?;
    out.extend(repeat_n(0.0, out_samples));
    let produced = {
        let input = InterleavedSlice::new(&decoded.samples[..], channels, frames)
            .expect("frame count was derived from the slice length");
        let mut output = InterleavedSlice::new_mut(&mut out[..], channels, out_frames)
            .expect("the block was allocated for exactly this many frames");
        let (_, produced) = resampler.process_all_into_buffer(&input, &mut output, frames, None)?;
        produced
    };
    out.truncate(produced * channels);
    AudioBuffer::new(decoded.channels, out).map_err(|(e, _)| e.into())
}

/// Refuses audio no [`AudioBuffer`] could hold, and returns its channel
/// count and frame count. Checked before anything is allocated, so a bad
/// shape costs nothing and the error names the real problem rather than
/// a buffer-size mismatch inside the resampler. A bad rate is the
/// resampler's to refuse.
fn check_shape(decoded: &DecodedAudio) -> Result<(usize, usize), ConvertError> {
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

/// A resampler from `rate` to [`SAMPLE_RATE`] for `channels` channels:
/// synchronous (the ratio is exact for the rate pair), anti-aliased, and
/// run over the clip in one call so its own delay is trimmed. The result
/// is as many frames as the clip would have had at the new rate, give or
/// take rounding, and starts where the clip starts.
fn resampler(rate: u32, channels: usize) -> Result<Fft<Pcm>, ConvertError> {
    Fft::<Pcm>::new(
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

    /// DecodedAudio audio at `rate`, in a block from a ledger that refuses
    /// nothing.
    fn at(rate: u32, channels: u16, samples: &[Pcm]) -> DecodedAudio {
        at_on(&Ledger::new(usize::MAX), rate, channels, samples)
    }

    fn at_on(ledger: &Ledger, rate: u32, channels: u16, samples: &[Pcm]) -> DecodedAudio {
        let mut block = ledger.alloc(samples.len()).unwrap();
        block.extend(samples.iter().copied());
        DecodedAudio {
            rate,
            channels,
            samples: block,
        }
    }

    /// Converts against a ledger that refuses nothing.
    fn convert(decoded: DecodedAudio) -> Result<AudioBuffer, ConvertError> {
        super::convert(decoded, &Ledger::new(usize::MAX))
    }

    fn peak(s: &[Pcm]) -> Pcm {
        s.iter().fold(0.0, |m, &x| m.max(x.abs()))
    }

    /// A sine of `hz` lasting `seconds` at `rate`, mono.
    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<Pcm> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    /// Zero crossings going upward: twice this per second is the frequency.
    fn rising_crossings(s: &[Pcm]) -> usize {
        s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn the_engine_rate_is_wrapped_in_the_block_it_arrived_in() {
        let ledger = Ledger::new(usize::MAX);
        let d = at_on(&ledger, SAMPLE_RATE, 2, &[0.5, -0.5, 0.25, -0.25]);
        let before = d.samples.as_ptr();
        let in_use = ledger.in_use();
        let b = super::convert(d, &ledger).unwrap();
        assert_eq!(b.channels(), 2);
        assert_eq!(b.frames(), 2);
        assert_eq!(b.samples(), &[0.5, -0.5, 0.25, -0.25]);
        assert_eq!(b.samples().as_ptr(), before);
        assert_eq!(ledger.in_use(), in_use);
    }

    #[test]
    fn a_bad_shape_is_refused_before_anything_is_paid_for() {
        let ledger = Ledger::new(usize::MAX);
        let d = at_on(&ledger, 44_100, 3, &[0.0; 6]);
        let in_use = ledger.in_use();
        assert!(matches!(
            super::convert(d, &ledger).unwrap_err(),
            ConvertError::Audio(AudioError::BadChannelCount(3))
        ));
        assert_eq!(ledger.in_use(), 0, "the decoded block is freed");
        assert!(in_use > 0);

        assert!(matches!(
            convert(at(44_100, 2, &[0.0; 3])).unwrap_err(),
            ConvertError::Audio(AudioError::RaggedFrames {
                samples: 3,
                channels: 2
            })
        ));
        assert!(matches!(
            convert(at(0, 1, &[0.0; 4])).unwrap_err(),
            ConvertError::Rate { rate: 0, .. }
        ));
    }

    #[test]
    fn resampling_that_does_not_fit_beside_the_clip_is_refused() {
        // Room for the decoded clip and nothing else.
        let samples = sine(44_100, 440.0, 0.1);
        let ledger = Ledger::new(samples.len() * size_of::<Pcm>());
        let d = at_on(&ledger, 44_100, 1, &samples);
        assert!(matches!(
            super::convert(d, &ledger).unwrap_err(),
            ConvertError::OverBudget(_)
        ));
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn other_rate_is_resampled_to_the_engine_rate() {
        let b = convert(at(44_100, 1, &sine(44_100, 440.0, 1.0))).unwrap();
        assert_eq!(b.channels(), 1);
        // One second in, one second out, within a frame or two of rounding.
        assert!(
            (b.frames() as i64 - SAMPLE_RATE as i64).abs() <= 2,
            "{}",
            b.frames()
        );
        // Same pitch: 440 rising crossings per second (edges may lose one).
        let out = b.samples();
        assert!((rising_crossings(out) as i64 - 440).abs() <= 2);
        // Same level: a pure tone's peak survives resampling.
        assert!((peak(out) - 0.5).abs() < 0.02, "{}", peak(out));
    }

    #[test]
    fn the_result_frees_the_clip_and_keeps_the_worst_case_paid_for() {
        let ledger = Ledger::new(usize::MAX);
        let samples = sine(44_100, 440.0, 1.0);
        let d = at_on(&ledger, 44_100, 1, &samples);
        let b = super::convert(d, &ledger).unwrap();
        // The clip is gone; what remains is the result's block, which is
        // at least its length.
        assert!(ledger.in_use() >= size_of_val(b.samples()));
        assert!(ledger.in_use() < samples.len() * size_of::<Pcm>() * 2);
    }

    #[test]
    fn resampling_keeps_channels_separate() {
        let left = sine(96_000, 100.0, 0.5);
        let interleaved: Vec<Pcm> = left.iter().flat_map(|&l| [l, 0.0]).collect();
        let b = convert(at(96_000, 2, &interleaved)).unwrap();
        assert_eq!(b.channels(), 2);
        assert!((b.frames() as i64 - 24_000).abs() <= 2, "{}", b.frames());
        let out = b.samples();
        let right: Vec<Pcm> = out.iter().skip(1).step_by(2).copied().collect();
        let left: Vec<Pcm> = out.iter().step_by(2).copied().collect();
        assert!(peak(&right) < 0.001, "{}", peak(&right));
        assert!((peak(&left) - 0.5).abs() < 0.02, "{}", peak(&left));
    }

    #[test]
    fn resampling_an_empty_clip_is_empty() {
        let b = convert(at(44_100, 2, &[])).unwrap();
        assert_eq!(b.frames(), 0);
        assert_eq!(b.channels(), 2);
    }
}
