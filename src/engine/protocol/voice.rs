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

/// A play's audio comes back with its reply; a stop's carries nothing.
impl Request for VoiceRequest {
    type Reply = Option<Arc<AudioBuffer>>;
    type Status = VoiceStatus;
}

/// What the audio thread last said about a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    /// Not reported ended yet: playing, or about to.
    Playing,
    Ended,
}
