//! A position moving through a loaded sample, first frame to last.
//!
//! A [`SampleReader`] holds which buffer and how far in. Its [`frames`]
//! are the next period of that buffer as stereo frames, and it moves on;
//! [`finished`] says when it has reached the end. It is a [`Source`].
//!
//! [`frames`]: Source::frames
//! [`finished`]: Source::finished

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::render::dsp::Source;
use crate::render::dsp::math::{Frame, sides_for, stereo};

/// A read position in a loaded sample, moving from its first frame to
/// its last at its own speed.
///
/// The position never passes the end of the buffer; that is the one
/// thing this type guards. Start and end points, looping and speed are
/// fields here, when they come; [`frames`](Source::frames) is where they
/// would act.
#[derive(Debug)]
pub struct SampleReader {
    audio: Arc<AudioBuffer>,
    /// The next frame to play.
    position: usize,
}

impl SampleReader {
    /// `audio` from its first frame.
    pub fn new(audio: Arc<AudioBuffer>) -> Self {
        Self { audio, position: 0 }
    }

    /// The buffer being played.
    pub fn audio(&self) -> &Arc<AudioBuffer> {
        &self.audio
    }

    /// The buffer it was made from, given back.
    pub fn into_audio(self) -> Arc<AudioBuffer> {
        self.audio
    }
}

impl Source for SampleReader {
    /// The next `frames` frames of the sample as stereo, mono on both
    /// sides, and the position moved past them. A sample with fewer
    /// frames left gives what it has.
    fn frames(&mut self, frames: usize) -> impl Iterator<Item = Frame> {
        let start = self.position;
        self.position += frames.min(self.audio.frames() - start);
        let layout = self.audio.layout();
        let samples = self.audio.frames_in(start..self.position);
        stereo(samples, layout.count(), sides_for(layout))
    }

    /// Whether every frame has been played, so `frames` will never give
    /// one again.
    fn finished(&self) -> bool {
        self.position >= self.audio.frames()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ChannelLayout::{self, Mono, Stereo};
    use crate::config::Pcm;
    use crate::testing::{audio, hear};

    const HALF: Pcm = 0.5;
    const ZERO: Pcm = 0.0;

    fn sample_reader(layout: ChannelLayout, data: Vec<Pcm>) -> SampleReader {
        SampleReader::new(Arc::new(audio(layout, &data)))
    }

    #[test]
    fn a_mono_sample_is_heard_on_both_channels() {
        let mut p = sample_reader(Mono, vec![HALF, -HALF]);
        let mut out = vec![0.0; 4];
        hear(&mut p, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
        assert!(p.finished());
    }

    #[test]
    fn a_stereo_sample_is_heard_as_stored() {
        let mut p = sample_reader(Stereo, vec![HALF, -HALF, ZERO, HALF]);
        let mut out = vec![0.0; 4];
        hear(&mut p, &mut out);
        assert_eq!(out, vec![0.5, -0.5, 0.0, 0.5]);
    }

    #[test]
    fn sample_readers_sum_by_adding() {
        let mut a = sample_reader(Mono, vec![HALF, HALF]);
        let mut b = sample_reader(Stereo, vec![HALF, ZERO, -HALF, ZERO]);
        let mut out = vec![0.0; 4];
        hear(&mut a, &mut out);
        hear(&mut b, &mut out);
        assert_eq!(out, vec![1.0, 0.5, 0.0, 0.5]);
    }

    #[test]
    fn a_long_sample_plays_across_periods() {
        let mut p = sample_reader(Mono, vec![HALF; 5]);
        let mut out = vec![0.0; 4];
        hear(&mut p, &mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        hear(&mut p, &mut out);
        assert!(!p.finished());

        let mut out = vec![0.0; 4];
        hear(&mut p, &mut out);
        // One frame left: the rest of the period is untouched.
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(p.finished());
    }

    #[test]
    fn a_finished_sample_reader_adds_nothing() {
        let mut p = sample_reader(Mono, vec![HALF]);
        let mut out = vec![0.0; 2];
        hear(&mut p, &mut out);
        let mut out = vec![0.25; 2];
        hear(&mut p, &mut out);
        assert_eq!(out, vec![0.25, 0.25]);
    }

    #[test]
    fn an_empty_sample_is_finished_at_once() {
        let p = sample_reader(Mono, vec![]);
        assert!(p.finished());
    }
}
