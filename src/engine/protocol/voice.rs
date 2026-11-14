//! What crosses for voices: playings of audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

use super::BufferAddress;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    Add(VoiceSpec),
    /// Removes the sender's voice; from any other sender, does nothing.
    Remove,
    /// Every voice playing this audio.
    RemoveAudio(BufferAddress),
    RemoveAll,
}

#[derive(Debug, Default)]
pub(crate) struct VoiceEvent {
    pub(crate) state: VoiceState,
}

/// Fixed for the voice's life.
#[derive(Debug)]
pub(crate) struct VoiceSpec {
    pub(crate) audio: Arc<AudioBuffer>,
}

/// What the audio thread last said about a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceState {
    /// Not reported ended yet: playing, or about to.
    #[default]
    Playing,
    Ended(VoiceEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEnd {
    /// It played to its last frame.
    Finished,
    /// Its `stop` arrived.
    Stopped,
    /// It was the oldest when a new voice needed its place.
    Cut,
}
