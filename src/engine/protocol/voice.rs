//! What crosses for voices: playings of audio.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::audio::AudioBuffer;

/// Shared by both sides while the voice plays.
#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    pub(crate) stop: AtomicBool,
}

/// Comes back whole once the voice ends, so its audio and state are freed
/// on the control side.
#[derive(Debug)]
pub(crate) struct VoiceData {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) state: Arc<VoiceState>,
}
