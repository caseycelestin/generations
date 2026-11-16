//! What crosses for voices: playings of audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::request::Request;

use super::BufferAddress;

#[derive(Debug)]
pub(crate) enum VoiceRequest {
    Play(Arc<AudioBuffer>),
    /// Every voice playing this audio.
    StopAudio(BufferAddress),
    StopAll,
}

impl Request for VoiceRequest {
    type Reply = ();
}

/// What the audio thread last said about a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Not reported ended yet: playing, or about to.
    Playing,
    Ended,
}
