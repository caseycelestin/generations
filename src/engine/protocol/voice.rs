//! What crosses for voices: playings of audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

use super::BufferAddress;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    Add(VoiceSpec),
    /// A stale id names nothing and does nothing.
    Remove(VoiceId),
    /// Every voice playing this audio.
    RemoveAudio(BufferAddress),
    RemoveAll,
}

#[derive(Debug, Default)]
pub(crate) struct VoiceEvent {
    pub(crate) state: VoiceState,
    /// Back only when the renderer lets go: on `Ended`.
    pub(crate) spec: Option<VoiceSpec>,
}

/// Fixed for the voice's life; the renderer keeps it whole beside its
/// playing state, to hand back.
#[derive(Debug)]
pub(crate) struct VoiceSpec {
    pub(crate) id: VoiceId,
    pub(crate) audio: Arc<AudioBuffer>,
}

id! {
    /// Lower is older, which is how the renderer finds the voice to cut.
    VoiceId
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
