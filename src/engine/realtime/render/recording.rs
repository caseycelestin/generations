//! The recording on the render side, while it runs.

use std::sync::Arc;

use rtrb::Producer;

use super::Frame;
use crate::engine::protocol::Return;
use crate::engine::protocol::recording::{RecordingEnd, RecordingSpec, RecordingState};

#[derive(Debug)]
pub(crate) struct Recording {
    spec: RecordingSpec,
}

impl Recording {
    pub(crate) fn start(spec: RecordingSpec) -> Self {
        spec.report.set(RecordingState::Recording);
        Self { spec }
    }

    /// Appends `block`, as heard, until the audio is full.
    pub(crate) fn write(&mut self, block: &[Frame]) {
        let audio = Arc::get_mut(&mut self.spec.audio);
        debug_assert!(audio.is_some(), "record refuses audio a voice holds");
        if let Some(audio) = audio {
            audio.extend(block.as_flattened().iter().copied());
        }
    }

    pub(crate) fn finished(&self) -> bool {
        self.spec.audio.is_full()
    }

    /// The one way a recording leaves: reports `why`, then hands its audio
    /// and report back to be freed on the control side, never here.
    pub(crate) fn end(self, why: RecordingEnd, to_control: &mut Producer<Return>) {
        self.spec.report.set(RecordingState::Ended(why));
        let pushed = to_control.push(Return::Recording(self.spec));
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }

    /// For a start that never became a recording: the recorder was busy.
    pub(crate) fn refuse(spec: RecordingSpec, to_control: &mut Producer<Return>) {
        spec.report.set(RecordingState::Refused);
        let pushed = to_control.push(Return::Recording(spec));
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}
