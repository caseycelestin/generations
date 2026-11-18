//! What crosses for voices: playings of audio.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::audio::AudioBuffer;
use crate::request::Request;

/// Shared by both sides while the voice plays.
#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    pub(crate) stop: AtomicBool,
}

#[derive(Debug)]
pub(crate) struct VoiceRequest {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) state: Arc<VoiceState>,
}

/// A play's audio comes back with its reply, to be freed on the control
/// side.
impl Request for VoiceRequest {
    type Reply = Arc<AudioBuffer>;
    type Status = VoiceStatus;
}

/// The renderer's answer, while it holds the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceStatus {
    Playing,
}
