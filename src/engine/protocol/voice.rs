//! What crosses for voices: playings of audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

use super::BufferAddress;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    Add(Arc<AudioBuffer>),
    /// Removes the sender's voice; from any other sender, does nothing.
    Remove,
    /// Every voice playing this audio.
    RemoveAudio(BufferAddress),
    RemoveAll,
}

/// What the audio thread last said about a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Not reported ended yet: playing, or about to.
    Playing,
    Ended(VoiceEnd),
}

/// The one event a voice sends: it says nothing when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEnd {
    /// It played to its last frame.
    Finished,
    /// Its `stop` arrived.
    Stopped,
    /// It was the oldest when a new voice needed its place.
    Cut,
}
