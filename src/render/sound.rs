//! One sound as the renderer plays it: a source and how it is heard.
//!
//! The control side's [`SampleSound`](crate::SampleSound) and
//! [`InputSound`](crate::InputSound) are the handles a user holds; this is
//! the same sound on the render side, either kind, told apart by its
//! module. It holds the [`Source`] that makes the frames and
//! the parameters the frames pass through on the way out, today a
//! [`Gain`], and it is a `Source` itself, so whoever runs it asks it for
//! frames and never sees the parts.

use crate::render::command::SoundParam;
use crate::render::dsp::math::Frame;
use crate::render::dsp::{Gain, Source};

/// A source and the parameters it is heard through.
#[derive(Debug)]
pub struct Sound<S> {
    source: S,
    gain: Gain,
}

impl<S> Sound<S> {
    /// `source`, heard at `gain`.
    pub fn new(source: S, gain: Gain) -> Self {
        Self { source, gain }
    }

    /// The source, to look at.
    pub fn source(&self) -> &S {
        &self.source
    }

    /// The source, given back.
    pub fn into_source(self) -> S {
        self.source
    }

    /// Moves one parameter to a new value.
    pub fn set(&mut self, param: SoundParam) {
        match param {
            SoundParam::Gain(level) => self.gain.set(level),
        }
    }
}

impl<S: Source> Source for Sound<S> {
    /// The source's next `frames` frames, through the gain.
    fn frames(&mut self, frames: usize) -> impl Iterator<Item = Frame> {
        self.gain.apply(frames, self.source.frames(frames))
    }

    /// Finished when its source is.
    fn finished(&self) -> bool {
        self.source.finished()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::ChannelLayout;
    use crate::render::dsp::SampleReader;
    use crate::testing::{audio, hear};

    fn half_sample(gain: f32) -> Sound<SampleReader> {
        let audio = Arc::new(audio(ChannelLayout::Mono, &[0.5; 4]));
        Sound::new(SampleReader::new(audio), Gain::at(gain))
    }

    #[test]
    fn the_source_is_heard_through_the_gain() {
        let mut s = half_sample(0.5);
        let mut out = vec![0.0; 4];
        hear(&mut s, &mut out);
        assert_eq!(out, vec![0.25; 4]);
    }

    #[test]
    fn a_set_gain_is_reached_by_the_end_of_the_next_period() {
        let mut s = half_sample(1.0);
        s.set(SoundParam::Gain(0.0));
        let mut out = vec![0.0; 4];
        hear(&mut s, &mut out);
        assert_eq!(out, vec![0.25, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn it_is_finished_when_its_source_is() {
        let mut s = half_sample(0.0);
        assert!(!s.finished());
        let mut out = vec![0.0; 8];
        hear(&mut s, &mut out);
        assert!(s.finished(), "silent, and still ran to the end");
    }
}
