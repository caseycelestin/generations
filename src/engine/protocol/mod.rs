//! What crosses between the two sides, owned by neither.
//!
//! Each request is a task the renderer carries out, answered once it is
//! done; handles stand for them on the control side. Only plain data
//! crosses: audio, ids and values. Each feature has its own file and its
//! own variant in [`Command`] and [`Event`], since each ring carries one
//! type.

use std::sync::Arc;

use crate::audio::AudioBuffer;

pub(crate) mod recording;
pub(crate) mod voice;

use recording::{RecordingEnd, RecordingRequest};
use voice::VoiceRequest;

#[derive(Debug)]
pub(crate) enum Command {
    Voice(VoiceRequest),
    Recording(RecordingRequest),
}

impl From<VoiceRequest> for Command {
    fn from(request: VoiceRequest) -> Self {
        Self::Voice(request)
    }
}

impl From<RecordingRequest> for Command {
    fn from(request: RecordingRequest) -> Self {
        Self::Recording(request)
    }
}

/// What happened in the renderer. A recording's last event carries its
/// audio back, to be taken; what nobody takes back goes to the collector.
#[derive(Debug)]
pub(crate) enum Event {
    Voice,
    Recording(RecordingEnd, Arc<AudioBuffer>),
}

/// A voice command's reply carries nothing.
impl TryFrom<Event> for () {
    type Error = Event;

    fn try_from(event: Event) -> Result<Self, Event> {
        match event {
            Event::Voice => Ok(()),
            other => Err(other),
        }
    }
}

impl TryFrom<Event> for (RecordingEnd, Arc<AudioBuffer>) {
    type Error = Event;

    fn try_from(event: Event) -> Result<Self, Event> {
        match event {
            Event::Recording(why, audio) => Ok((why, audio)),
            other => Err(other),
        }
    }
}

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
