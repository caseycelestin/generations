//! Frames written into an audio buffer, first frame to last, until it is
//! full.
//!
//! A [`Recorder`] is the other end of a [`Source`](crate::render::dsp::Source):
//! a source gives frames, a recorder keeps them. It owns one stereo
//! [`AudioBuffer`], paid for on the control side when the recorder is
//! made, and appends each period's frames to it. A full recorder keeps
//! nothing more; being full is not an error. What it kept is a buffer
//! like any loaded sample's, so a take is saved straight from it.
//!
//! A recorder is one take. It is built around a buffer when the take
//! starts, which forgets what the buffer held, and taken apart when the
//! take ends, handing the buffer back with the take in it. The same
//! buffer goes round again for the next take, so a take allocates
//! nothing.

use crate::audio::{AudioBuffer, ChannelLayout};
#[cfg(test)]
use crate::config::Pcm;
use crate::render::dsp::math::Frame;

/// A stereo audio buffer, filled from the first frame on.
#[derive(Debug)]
pub struct Recorder {
    audio: AudioBuffer,
}

impl Recorder {
    /// Starts a take in `audio`, which must be stereo: whatever it held
    /// is forgotten and its room comes back. It is paid for by whoever
    /// made it, on the control side; this only fills it, so it may be
    /// built on the audio thread.
    pub fn new(mut audio: AudioBuffer) -> Self {
        debug_assert_eq!(audio.layout(), ChannelLayout::Stereo, "frames are stereo");
        audio.clear();
        Self { audio }
    }

    /// Keeps `frames` after what is already kept, up to the room left.
    /// Frames past the room are dropped.
    pub fn record(&mut self, frames: impl IntoIterator<Item = Frame>) {
        for frame in frames.into_iter().take(self.audio.room()) {
            self.audio.push_frame(&frame);
        }
    }

    /// Whether it has no room for another frame.
    pub fn full(&self) -> bool {
        self.audio.room() == 0
    }

    /// The take: the buffer, holding what it kept.
    pub fn into_audio(self) -> AudioBuffer {
        self.audio
    }

    /// The frames kept so far, interleaved.
    #[cfg(test)] // read by nothing but tests until a take is saved
    pub fn recorded(&self) -> &[Pcm] {
        self.audio.samples()
    }
}

#[cfg(test)]
mod tests {
    use super::Recorder;
    use crate::testing::recorder;

    #[test]
    fn a_new_recorder_is_empty() {
        let recorder = recorder(4);
        assert!(recorder.recorded().is_empty());
        assert!(!recorder.full());
    }

    #[test]
    fn frames_are_kept_in_order_across_periods() {
        let mut recorder = recorder(4);
        recorder.record([[0.1, 0.2], [0.3, 0.4]]);
        recorder.record([[0.5, 0.6]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert!(!recorder.full());
    }

    #[test]
    fn frames_past_the_room_are_dropped() {
        let mut recorder = recorder(2);
        recorder.record([[0.1, 0.2], [0.3, 0.4], [0.5, 0.6]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4]);
        assert!(recorder.full());
        recorder.record([[0.7, 0.8]]);
        assert_eq!(recorder.recorded(), &[0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn a_new_take_in_the_same_buffer_forgets_the_last() {
        let mut first = recorder(1);
        first.record([[0.1, 0.2]]);
        assert!(first.full());
        let mut second = Recorder::new(first.into_audio());
        assert!(second.recorded().is_empty());
        assert!(!second.full());
        second.record([[0.3, 0.4]]);
        assert_eq!(second.recorded(), &[0.3, 0.4]);
    }
}
