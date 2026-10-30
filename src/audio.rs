//! Raw PCM storage.
//!
//! An [`AudioBuffer`] is dumb, immutable data: an interleaved block of
//! [`Pcm`] samples with a channel count. It has no identity and knows nothing
//! about where it came from. Processing happens in `f32`; the two conversion
//! functions here are the only code that knows what `Pcm` actually is.

use std::fmt;

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};
use thiserror::Error;

use crate::config::{MAX_CHANNELS, Pcm};

/// Full-scale magnitude of the PCM type, as a float.
const SCALE: f32 = 32_768.0;

/// Reasons a buffer cannot be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AudioError {
    /// `channels` was zero or above [`MAX_CHANNELS`].
    #[error("channel count {0} is not in 1..={MAX_CHANNELS}")]
    BadChannelCount(u16),
    /// The sample count was not a whole number of frames.
    #[error("{samples} samples is not a whole number of {channels}-channel frames")]
    RaggedFrames { samples: usize, channels: u16 },
}

/// Interleaved PCM audio at the engine's fixed sample rate.
#[derive(Clone, PartialEq, Eq)]
pub struct AudioBuffer {
    channels: u16,
    data: Box<[Pcm]>,
}

impl AudioBuffer {
    /// Wraps interleaved samples, checking the frame invariant.
    pub fn new(channels: u16, data: impl Into<Box<[Pcm]>>) -> Result<Self, AudioError> {
        let data = data.into();
        check_channels(channels)?;
        if data.len() % usize::from(channels) != 0 {
            return Err(AudioError::RaggedFrames { samples: data.len(), channels });
        }
        Ok(Self { channels, data })
    }

    /// Builds a buffer from normalised `f32` samples in `-1.0..=1.0`.
    ///
    /// Input is clamped to full scale, dithered with triangular noise, and
    /// rounded to the nearest PCM value. This is the only way audio enters
    /// storage from processing, so it is where quantisation happens.
    pub fn from_f32(channels: u16, samples: &[f32]) -> Result<Self, AudioError> {
        check_channels(channels)?;
        let mut dither = Dither::default();
        let data: Vec<Pcm> = samples
            .iter()
            .map(|&s| {
                let scaled = s.clamp(-1.0, 1.0) * SCALE + dither.next();
                scaled.round().clamp(Pcm::MIN as f32, Pcm::MAX as f32) as Pcm
            })
            .collect();
        Self::new(channels, data)
    }

    /// Converts to normalised `f32` samples, still interleaved.
    pub fn to_f32(&self) -> Vec<f32> {
        self.data.iter().map(|&s| f32::from(s) / SCALE).collect()
    }

    /// Number of interleaved channels.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Number of frames, where one frame holds one sample per channel.
    pub fn frames(&self) -> usize {
        self.data.len() / usize::from(self.channels)
    }

    /// All samples, interleaved.
    pub fn samples(&self) -> &[Pcm] {
        &self.data
    }

}

impl fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBuffer")
            .field("channels", &self.channels)
            .field("frames", &self.frames())
            .finish_non_exhaustive()
    }
}

fn check_channels(channels: u16) -> Result<(), AudioError> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::BadChannelCount(channels));
    }
    Ok(())
}

/// Triangular (TPDF) dither in the range `-1.0..1.0` LSB.
///
/// Backed by `rand`'s `SmallRng` with a fixed seed, so conversion is
/// deterministic for a given input.
struct Dither {
    rng: SmallRng,
}

impl Default for Dither {
    fn default() -> Self {
        Self { rng: SmallRng::seed_from_u64(0x9E37_79B9) }
    }
}

impl Dither {
    /// Sum of two uniforms, centred: triangular on `-1.0..1.0`.
    fn next(&mut self) -> f32 {
        self.rng.random::<f32>() + self.rng.random::<f32>() - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_channel_counts() {
        assert_eq!(AudioBuffer::new(0, vec![]).unwrap_err(), AudioError::BadChannelCount(0));
        assert_eq!(AudioBuffer::new(3, vec![]).unwrap_err(), AudioError::BadChannelCount(3));
    }

    #[test]
    fn rejects_ragged_frames() {
        let err = AudioBuffer::new(2, vec![0, 0, 0]).unwrap_err();
        assert_eq!(err, AudioError::RaggedFrames { samples: 3, channels: 2 });
    }

    #[test]
    fn frames_count_per_channel() {
        let b = AudioBuffer::new(2, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(b.frames(), 2);
        assert_eq!(b.channels(), 2);
        assert_eq!(b.samples(), &[1, 2, 3, 4]);
    }

    #[test]
    fn f32_round_trip_is_within_dither() {
        let b = AudioBuffer::new(1, vec![0, 1000, -1000, Pcm::MAX, Pcm::MIN]).unwrap();
        let back = AudioBuffer::from_f32(1, &b.to_f32()).unwrap();
        for (a, b) in b.samples().iter().zip(back.samples()) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 2, "{a} vs {b}");
        }
    }

    #[test]
    fn from_f32_clamps_out_of_range() {
        let b = AudioBuffer::from_f32(1, &[2.0, -2.0]).unwrap();
        assert!(b.samples()[0] >= Pcm::MAX - 1);
        assert!(b.samples()[1] <= Pcm::MIN + 1);
    }

    #[test]
    fn dither_is_triangular_and_bounded() {
        let mut d = Dither::default();
        let mut sum = 0.0f64;
        for _ in 0..10_000 {
            let v = d.next();
            assert!((-1.0..1.0).contains(&v));
            sum += f64::from(v);
        }
        assert!((sum / 10_000.0).abs() < 0.02);
    }
}
