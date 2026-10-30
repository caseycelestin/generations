//! Offline effects: pure functions from buffer to buffer.
//!
//! An effect takes an [`AudioBuffer`] and returns a new one. It knows nothing
//! about projects, samples or lineage; printing the result is the engine's
//! job. Nothing here runs in real time.
//!
//! Effects that change sample values work in `f32` and convert through
//! [`AudioBuffer::to_f32`] / [`AudioBuffer::from_f32`]. Effects that only
//! reorder frames, like [`reverse`], work on the PCM data directly so that
//! they are exact and do not pay for a dither pass.

use crate::audio::AudioBuffer;

/// Plays the audio backwards.
///
/// Frames are reversed as units, so channels stay in their lanes. Reversing
/// twice gives back the original bytes exactly.
pub fn reverse(input: &AudioBuffer) -> AudioBuffer {
    let channels = input.channels();
    let data: Vec<_> = input
        .samples()
        .chunks(usize::from(channels))
        .rev()
        .flatten()
        .copied()
        .collect();
    AudioBuffer::new(channels, data).expect("same channel count and length as input")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_mono_reverses_samples() {
        let b = AudioBuffer::new(1, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(reverse(&b).samples(), &[4, 3, 2, 1]);
    }

    #[test]
    fn reverse_stereo_keeps_channels_in_lane() {
        // Frames: (1,-1) (2,-2) (3,-3). Left stays positive, right negative.
        let b = AudioBuffer::new(2, vec![1, -1, 2, -2, 3, -3]).unwrap();
        let r = reverse(&b);
        assert_eq!(r.channels(), 2);
        assert_eq!(r.samples(), &[3, -3, 2, -2, 1, -1]);
    }

    #[test]
    fn reverse_twice_is_identity() {
        let b = AudioBuffer::new(2, (0..64).map(|i| i * 37 - 1000).collect::<Vec<_>>()).unwrap();
        assert_eq!(reverse(&reverse(&b)), b);
    }

    #[test]
    fn reverse_of_empty_is_empty() {
        let b = AudioBuffer::new(2, vec![]).unwrap();
        assert_eq!(reverse(&b).frames(), 0);
    }
}
