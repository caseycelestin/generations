//! The recording on the render side, while it runs.

use std::sync::Arc;

use rtrb::Producer;

use super::Frame;
use crate::engine::protocol::recording::{
    RecordingEnd, RecordingEvent, RecordingSpec, RecordingState,
};
use crate::engine::protocol::{Event, HandleId, Message};

#[derive(Debug)]
pub(crate) struct Recording {
    id: HandleId,
    spec: RecordingSpec,
}

impl Recording {
    pub(crate) fn start(
        id: HandleId,
        spec: RecordingSpec,
        to_control: &mut Producer<Message<Event>>,
    ) -> Self {
        let event = RecordingEvent {
            state: RecordingState::Recording,
            spec: None,
        };
        let pushed = to_control.push(Message {
            id,
            data: event.into(),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
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

    pub(crate) fn is(&self, id: HandleId) -> bool {
        self.id == id
    }

    pub(crate) fn finished(&self) -> bool {
        self.spec.audio.is_full()
    }

    /// The one way a recording leaves.
    pub(crate) fn end(self, why: RecordingEnd, to_control: &mut Producer<Message<Event>>) {
        let event = RecordingEvent {
            state: RecordingState::Ended(why),
            spec: Some(self.spec),
        };
        let pushed = to_control.push(Message {
            id: self.id,
            data: event.into(),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }

    pub(crate) fn refuse(
        id: HandleId,
        spec: RecordingSpec,
        to_control: &mut Producer<Message<Event>>,
    ) {
        let event = RecordingEvent {
            state: RecordingState::Refused,
            spec: Some(spec),
        };
        let pushed = to_control.push(Message {
            id,
            data: event.into(),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}
