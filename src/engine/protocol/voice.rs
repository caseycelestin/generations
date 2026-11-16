//! What crosses for voices: playings of audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::request::Request;

#[derive(Debug)]
pub(crate) enum VoiceRequest {
    Play(Arc<AudioBuffer>),
    /// Every voice playing this audio.
    StopAudio(Arc<AudioBuffer>),
    StopAll,
}

/// What a request carried comes back with its reply, to be freed on the
/// control side: a play's audio, or the audio a stop named. A stop-all
/// carries nothing.
impl Request for VoiceRequest {
    type Reply = Option<Arc<AudioBuffer>>;
    type Status = VoiceStatus;
}

/// The renderer's answer, while it holds the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Playing,
    Ended,
}
