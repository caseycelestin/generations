//! The recording on the render side, while it runs.

use std::sync::Arc;

use super::Frame;
use crate::duplex::{Id, RealtimeEnd};
use crate::engine::protocol::recording::{
    RecordingEnd, RecordingEvent, RecordingSpec, RecordingState,
};
use crate::engine::protocol::{Command, Event};

#[derive(Debug)]
pub(crate) struct Recording {
    id: Id,
    spec: RecordingSpec,
}

impl Recording {
    pub(crate) fn start(
        id: Id,
        spec: RecordingSpec,
        channel: &mut RealtimeEnd<Command, Event>,
    ) -> Self {
        let event = RecordingEvent {
            state: RecordingState::Recording,
            spec: None,
        };
        let sent = channel.send(id, event);
        debug_assert!(sent.is_ok(), "a recording reserves its start");
        Self { id, spec }
    }

    /// Appends `block`, as heard, until the audio is full.
    pub(crate) fn write(&mut self, block: &[Frame]) {
        let audio = Arc::get_mut(&mut self.spec.audio);
        debug_assert!(audio.is_some(), "record refuses audio a voice holds");
        if let Some(audio) = audio {
            audio.extend(block.as_flattened().iter().copied());
        }
    }

    pub(crate) fn is(&self, id: Id) -> bool {
        self.id == id
    }

    pub(crate) fn finished(&self) -> bool {
        self.spec.audio.is_full()
    }

    /// The one way a recording leaves.
    pub(crate) fn end(self, why: RecordingEnd, channel: &mut RealtimeEnd<Command, Event>) {
        let event = RecordingEvent {
            state: RecordingState::Ended(why),
            spec: Some(self.spec),
        };
        let ended = channel.end(self.id, event);
        debug_assert!(ended.is_ok(), "a recording reserves its end");
    }

    pub(crate) fn refuse(id: Id, spec: RecordingSpec, channel: &mut RealtimeEnd<Command, Event>) {
        let event = RecordingEvent {
            state: RecordingState::Refused,
            spec: Some(spec),
        };
        let ended = channel.end(id, event);
        debug_assert!(ended.is_ok(), "a recording reserves its end");
    }
}
