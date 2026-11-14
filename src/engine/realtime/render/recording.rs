//! The recording on the render side, while it runs.

use std::sync::Arc;

use super::Frame;
use crate::audio::AudioBuffer;
use crate::duplex::{Id, RealtimeEnd};
use crate::engine::protocol::recording::{RecordingEnd, RecordingEvent};
use crate::engine::protocol::{Command, Event};

#[derive(Debug)]
pub(crate) struct Recording {
    id: Id,
    audio: Arc<AudioBuffer>,
}

impl Recording {
    pub(crate) fn start(
        id: Id,
        audio: Arc<AudioBuffer>,
        channel: &mut RealtimeEnd<Command, Event>,
    ) -> Self {
        let sent = channel.try_send(id, RecordingEvent::Started);
        debug_assert!(sent.is_ok(), "a recording reserves its start");
        Self { id, audio }
    }

    /// Appends `block`, as heard, until the audio is full.
    pub(crate) fn write(&mut self, block: &[Frame]) {
        let audio = Arc::get_mut(&mut self.audio);
        debug_assert!(audio.is_some(), "record refuses audio a voice holds");
        if let Some(audio) = audio {
            audio.extend(block.as_flattened().iter().copied());
        }
    }

    pub(crate) fn is(&self, id: Id) -> bool {
        self.id == id
    }

    pub(crate) fn finished(&self) -> bool {
        self.audio.is_full()
    }

    /// The one way a recording leaves.
    pub(crate) fn end(self, why: RecordingEnd, channel: &mut RealtimeEnd<Command, Event>) {
        let ended = channel.end(self.id, RecordingEvent::Ended(why, self.audio));
        debug_assert!(ended.is_ok(), "a recording reserves its end");
    }

    pub(crate) fn refuse(
        id: Id,
        audio: Arc<AudioBuffer>,
        channel: &mut RealtimeEnd<Command, Event>,
    ) {
        let ended = channel.end(id, RecordingEvent::Refused(audio));
        debug_assert!(ended.is_ok(), "a recording reserves its end");
    }
}
