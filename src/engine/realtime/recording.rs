//! The recording on the render side, while it runs.

use std::sync::Arc;

use super::Frame;
use crate::audio::AudioBuffer;
use crate::engine::protocol::recording::{RecordingEnd, RecordingRequest};
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Recording {
    permit: ReplyPermit,
    audio: Arc<AudioBuffer>,
}

impl Recording {
    pub(crate) fn start(permit: ReplyPermit, audio: Arc<AudioBuffer>) -> Self {
        Self { permit, audio }
    }

    /// Appends `block`, as heard, until the audio is full.
    pub(crate) fn write(&mut self, block: &[Frame]) {
        let audio = Arc::get_mut(&mut self.audio);
        debug_assert!(audio.is_some(), "record refuses audio a voice holds");
        if let Some(audio) = audio {
            audio.extend(block.as_flattened().iter().copied());
        }
    }

    pub(crate) fn is(&self, id: RequestId) -> bool {
        self.permit.id() == id
    }

    pub(crate) fn finished(&self) -> bool {
        self.audio.is_full()
    }

    /// The one way a recording leaves: its audio goes back in the reply.
    pub(crate) fn end(self, why: RecordingEnd, requests: &mut RealtimeEnd<RecordingRequest>) {
        requests.close(self.permit, (why, self.audio));
    }
}
