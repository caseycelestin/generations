//! A position moving through a loaded sample, first frame to last.
//!
//! A [`Playhead`] holds which buffer and how far in. Its [`add`] adds the
//! next period of that buffer into the output and moves on; [`finished`]
//! says when it has reached the end. It is a [`Source`], held in the
//! renderer's sample mixer, which calls it once a period.
//!
//! [`add`]: Source::add
//! [`finished`]: Source::finished

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::config::{OUTPUT_CHANNELS, Pcm};
use crate::dsp::Source;
use crate::dsp::math::{add, sides_for, stereo};

/// A read position in a loaded sample, moving from its first frame to
/// its last at its own speed.
///
/// The position never passes the end of the buffer; that is the one
/// thing this type guards. Start and end points, looping and speed are
/// fields here, when they come; [`add`](Source::add) is where they would
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
}

impl Source for Playhead {
    /// Adds the next period into `out`, an interleaved stereo slice, and
    /// advances past it. A mono sample is added to both channels. A
    /// sample with fewer frames left than `out` holds adds what it has
    /// and leaves the rest untouched. Nothing is clipped here.
    fn add(&mut self, out: &mut [Pcm]) {
        debug_assert_eq!(out.len() % OUTPUT_CHANNELS, 0, "output is not whole frames");
        let frames = (out.len() / OUTPUT_CHANNELS).min(self.audio.frames() - self.position);
        let samples = self.audio.frames_in(self.position..self.position + frames);
        let channels = self.audio.channels();
        add(out, stereo(samples, channels.into(), sides_for(channels)));
        self.position += frames;
    }

    /// Moves past the next `frames` frames unheard, never past the end.
    fn skip(&mut self, frames: usize) {
        self.position = (self.position + frames).min(self.audio.frames());
    }

    /// Whether every frame has been played, so `add` will never add
    /// anything again.
    fn finished(&self) -> bool {
        self.position >= self.audio.frames()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        p.add(&mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
        assert!(p.finished());
    }

    #[test]
    fn a_stereo_sample_is_heard_as_stored() {
        let mut p = playhead(2, vec![HALF, -HALF, ZERO, HALF]);
        let mut out = vec![0.0; 4];
        p.add(&mut out);
        assert_eq!(out, vec![0.5, -0.5, 0.0, 0.5]);
    }

    #[test]
    fn playheads_sum_by_adding() {
        let mut a = playhead(1, vec![HALF, HALF]);
        let mut b = playhead(2, vec![HALF, ZERO, -HALF, ZERO]);
        let mut out = vec![0.0; 4];
        a.add(&mut out);
        b.add(&mut out);
        assert_eq!(out, vec![1.0, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn a_long_sample_plays_across_periods() {
        let mut p = playhead(1, vec![HALF; 5]);
        let mut out = vec![0.0; 4];
        p.add(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        p.add(&mut out);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        p.add(&mut out);
        // One frame left: the rest of the period is untouched.
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(p.finished());
    }

    #[test]
    fn a_finished_playhead_adds_nothing() {
        let mut p = playhead(1, vec![HALF]);
        let mut out = vec![0.0; 2];
        p.add(&mut out);
        let mut out = vec![0.25; 2];
        p.add(&mut out);
        assert_eq!(out, vec![0.25, 0.25]);
    }

    #[test]
    fn a_skip_moves_on_unheard_and_stops_at_the_end() {
        let mut p = playhead(1, vec![HALF, -HALF, HALF]);
        p.skip(1);
        let mut out = vec![0.0; 2];
        p.add(&mut out);
        assert_eq!(out, vec![-0.5, -0.5]);
        p.skip(10);
        assert!(p.finished());
    }

    #[test]
    fn an_empty_sample_is_finished_at_once() {
        let p = playhead(1, vec![]);
        assert!(p.finished());
    }
}
