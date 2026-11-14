//! What crosses between the two sides, owned by neither.
//!
//! Commands are operations on what the renderer holds (add, remove), not
//! user intents; handles translate one into the other. Only plain data
//! crosses: audio, ids and values. Each feature has its own file and its
//! own variant in [`Command`] and [`Event`], since each ring carries one
//! type.

use std::sync::Arc;

use crate::audio::AudioBuffer;

pub(crate) mod recording;
pub(crate) mod voice;

use recording::{RecordingCommand, RecordingEvent};
use voice::{VoiceCommand, VoiceEvent};

/// What a ring carries: whose it is, and the data for it.
#[derive(Debug)]
pub(crate) struct Message<T> {
    pub(crate) id: HandleId,
    pub(crate) data: T,
}

#[derive(Debug)]
pub(crate) enum Command {
    Voice(VoiceCommand),
    Recording(RecordingCommand),
}

impl From<VoiceCommand> for Command {
    fn from(command: VoiceCommand) -> Self {
        Self::Voice(command)
    }
}

impl From<RecordingCommand> for Command {
    fn from(command: RecordingCommand) -> Self {
        Self::Recording(command)
    }
}

/// What happened in the renderer. A spec in it is handed back so its last
/// reference is dropped on the control side: freeing has no time bound and
/// must not happen on the audio thread.
#[derive(Debug)]
pub(crate) enum Event {
    Voice(VoiceEvent),
    Recording(RecordingEvent),
}

impl From<VoiceEvent> for Event {
    fn from(event: VoiceEvent) -> Self {
        Self::Voice(event)
    }
}

impl From<RecordingEvent> for Event {
    fn from(event: RecordingEvent) -> Self {
        Self::Recording(event)
    }
}

/// Names one handle's thing (a voice, a take) on both sides: what the
/// renderer sends back about it carries this id. Unique within one engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HandleId(pub(crate) u64);

/// Names audio to the renderer without sending it, so nothing comes back
/// to be freed. Sound only while the audio cannot be freed before the
/// command is applied: a later buffer at the same address then starts
/// behind it on the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferAddress(usize);

impl BufferAddress {
    pub(crate) fn of(audio: &Arc<AudioBuffer>) -> Self {
        Self(Arc::as_ptr(audio) as usize)
    }
}
