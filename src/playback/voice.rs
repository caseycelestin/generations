//! One sample playing.
//!
//! A [`Voice`] is how a sample is heard: today, the whole buffer from the
//! first frame to the last at its own speed. Start and end points, looping,
//! gain and fades are all fields and lines here, when they come; the mixer
//! that sums voices never needs to know about them.
//!
//! Runs on the real-time thread. Nothing here allocates, locks or fails.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::config::OUTPUT_CHANNELS;
use crate::playback::renderer::VoiceId;

/// A sample playing from the first frame to the last.
#[derive(Debug)]
pub struct Voice {
    id: VoiceId,
    audio: Arc<AudioBuffer>,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    /// Voice `id` at the start of `audio`.
    pub fn new(id: VoiceId, audio: Arc<AudioBuffer>) -> Self {
        Self {
            id,
            audio,
            position: 0,
        }
    }

    /// Which started sound this is.
    pub fn id(&self) -> VoiceId {
        self.id
    }

    /// Adds this voice's next frames into `out`, an interleaved stereo
    /// slice, and returns whether the voice has now reached its end.
    ///
    /// Adds rather than writes, so voices sum by each calling this on the
    /// same slice. A mono sample is added to both channels. A voice with
    /// fewer frames left than `out` holds adds what it has and leaves the
    /// rest untouched. Nothing is clipped here; the sum is clipped once.
    pub fn mix_into(&mut self, out: &mut [f32]) -> bool {
        debug_assert_eq!(out.len() % OUTPUT_CHANNELS, 0, "output is not whole frames");
        let frames = (out.len() / OUTPUT_CHANNELS).min(self.audio.frames() - self.position);
        let range = self.position..self.position + frames;
        match self.audio.channels() {
            1 => {
                let (frames, _) = out.as_chunks_mut::<OUTPUT_CHANNELS>();
                for (frame, s) in frames.iter_mut().zip(self.audio.frames_f32(range)) {
                    for o in frame {
                        *o += s;
                    }
                }
            }
            _ => {
                for (o, s) in out.iter_mut().zip(self.audio.frames_f32(range)) {
                    *o += s;
                }
            }
        }
        self.position += frames;
        self.finished()
    }

    /// Whether every frame has been played.
    pub fn finished(&self) -> bool {
        self.position >= self.audio.frames()
    }

    /// The audio this voice plays.
    pub fn audio(&self) -> &Arc<AudioBuffer> {
        &self.audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Pcm;

    /// Half scale, which converts to exactly 0.5.
    const HALF: Pcm = 16_384;

    fn voice(channels: u16, data: Vec<Pcm>) -> Voice {
        Voice::new(
            VoiceId(0),
            Arc::new(AudioBuffer::new(channels, data).unwrap()),
        )
    }

    #[test]
    fn a_mono_sample_is_heard_on_both_channels() {
        let mut v = voice(1, vec![HALF, -HALF]);
        let mut out = vec![0.0; 4];
        assert!(v.mix_into(&mut out));
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn a_stereo_sample_is_heard_as_stored() {
        let mut v = voice(2, vec![HALF, -HALF, 0, HALF]);
        let mut out = vec![0.0; 4];
        assert!(v.mix_into(&mut out));
        assert_eq!(out, vec![0.5, -0.5, 0.0, 0.5]);
    }

    #[test]
    fn mixing_adds_to_what_is_already_there() {
        let mut a = voice(1, vec![HALF, HALF]);
        let mut b = voice(2, vec![HALF, 0, -HALF, 0]);
        let mut out = vec![0.0; 4];
        a.mix_into(&mut out);
        b.mix_into(&mut out);
        assert_eq!(out, vec![1.0, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn a_long_sample_plays_across_periods() {
        let mut v = voice(1, vec![HALF; 5]);
        let mut out = vec![0.0; 4];
        assert!(!v.mix_into(&mut out));
        assert_eq!(out, vec![0.5; 4]);
        assert!(!v.finished());

        let mut out = vec![0.0; 4];
        assert!(!v.mix_into(&mut out));
        assert!(!v.finished());

        let mut out = vec![0.0; 4];
        assert!(v.mix_into(&mut out));
        // One frame left: the rest of the period is untouched.
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(v.finished());
    }

    #[test]
    fn a_finished_voice_adds_nothing() {
        let mut v = voice(1, vec![HALF]);
        let mut out = vec![0.0; 2];
        assert!(v.mix_into(&mut out));
        let mut out = vec![0.25; 2];
        assert!(v.mix_into(&mut out));
        assert_eq!(out, vec![0.25, 0.25]);
    }

    #[test]
    fn an_empty_sample_is_finished_at_once() {
        let v = voice(1, vec![]);
        assert!(v.finished());
    }
}
