//! A position moving through a loaded sample, first frame to last.
//!
//! A [`Playhead`] holds which buffer and how far in. Its [`play`] adds the
//! next period of that buffer into the output and moves on; [`finished`]
//! says when it has reached the end. The wiring owns the value and calls
//! it once a period.
//!
//! [`play`]: Playhead::play
//! [`finished`]: Playhead::finished

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::config::OUTPUT_CHANNELS;

/// A read position in a loaded sample, moving from its first frame to
/// its last at its own speed.
///
/// The position never passes the end of the buffer; that is the one
/// thing this type guards. Start and end points, looping and speed are
/// fields here, when they come; [`play`](Self::play) is where they would
/// act.
#[derive(Debug)]
pub struct Playhead {
    audio: Arc<AudioBuffer>,
    /// The next frame to play.
    position: usize,
}

impl Playhead {
    /// `audio` from its first frame.
    pub fn new(audio: Arc<AudioBuffer>) -> Self {
        Self { audio, position: 0 }
    }

    /// The buffer being played.
    pub fn audio(&self) -> &Arc<AudioBuffer> {
        &self.audio
    }

    /// Whether every frame has been played, so [`play`](Self::play) will
    /// never add anything again.
    pub fn finished(&self) -> bool {
        self.position >= self.audio.frames()
    }

    /// Adds the next period into `out`, an interleaved stereo slice, and
    /// advances past it. A mono sample is added to both channels. A
    /// sample with fewer frames left than `out` holds adds what it has
    /// and leaves the rest untouched. Nothing is clipped here.
    ///
    /// Adds rather than writes, so sounds sum by each being played into
    /// the same slice.
    pub fn play(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len() % OUTPUT_CHANNELS, 0, "output is not whole frames");
        let frames = (out.len() / OUTPUT_CHANNELS).min(self.audio.frames() - self.position);
        let range = self.position..self.position + frames;
        match self.audio.channels() {
            1 => {
                let (frames, _) = out.as_chunks_mut::<OUTPUT_CHANNELS>();
                for (frame, &s) in frames.iter_mut().zip(self.audio.frames_in(range)) {
                    for o in frame {
                        *o += s;
                    }
                }
            }
            _ => {
                for (o, &s) in out.iter_mut().zip(self.audio.frames_in(range)) {
                    *o += s;
                }
            }
        }
        self.position += frames;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Pcm;
    use crate::testing::audio;

    const HALF: Pcm = 0.5;
    const ZERO: Pcm = 0.0;

    fn playhead(channels: u16, data: Vec<Pcm>) -> Playhead {
        Playhead::new(Arc::new(audio(channels, &data)))
    }

    #[test]
    fn a_mono_sample_is_heard_on_both_channels() {
        let mut p = playhead(1, vec![HALF, -HALF]);
        let mut out = vec![0.0; 4];
        p.play(&mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
        assert!(p.finished());
    }

    #[test]
    fn a_stereo_sample_is_heard_as_stored() {
        let mut p = playhead(2, vec![HALF, -HALF, ZERO, HALF]);
        let mut out = vec![0.0; 4];
        p.play(&mut out);
        assert_eq!(out, vec![0.5, -0.5, 0.0, 0.5]);
    }

    #[test]
    fn playheads_sum_by_adding() {
        let mut a = playhead(1, vec![HALF, HALF]);
        let mut b = playhead(2, vec![HALF, ZERO, -HALF, ZERO]);
        let mut out = vec![0.0; 4];
        a.play(&mut out);
        b.play(&mut out);
        assert_eq!(out, vec![1.0, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn a_long_sample_plays_across_periods() {
        let mut p = playhead(1, vec![HALF; 5]);
        let mut out = vec![0.0; 4];
        p.play(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        p.play(&mut out);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        p.play(&mut out);
        // One frame left: the rest of the period is untouched.
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(p.finished());
    }

    #[test]
    fn a_finished_playhead_adds_nothing() {
        let mut p = playhead(1, vec![HALF]);
        let mut out = vec![0.0; 2];
        p.play(&mut out);
        let mut out = vec![0.25; 2];
        p.play(&mut out);
        assert_eq!(out, vec![0.25, 0.25]);
    }

    #[test]
    fn an_empty_sample_is_finished_at_once() {
        assert!(playhead(1, vec![]).finished());
    }
}
