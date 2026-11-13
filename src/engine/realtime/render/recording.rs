//! The recording on the render side, while it runs.

use std::sync::Arc;

use rtrb::Producer;

use super::Frame;
use crate::engine::protocol::HandleId;
use crate::engine::protocol::Return;
use crate::engine::protocol::recording::{RecordingEnd, RecordingSpec, Returned};

#[derive(Debug)]
pub(crate) struct Recording {
    spec: RecordingSpec,
}

impl Recording {
    pub(crate) fn start(spec: RecordingSpec, to_control: &mut Producer<Return>) -> Self {
        let pushed = to_control.push(Return::Recording {
            id: spec.id,
            state: Returned::Started,
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
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

    pub(crate) fn is(&self, id: HandleId) -> bool {
        self.spec.id == id
    }

    pub(crate) fn finished(&self) -> bool {
        self.spec.audio.is_full()
    }

    /// The one way a recording leaves.
    pub(crate) fn end(self, why: RecordingEnd, to_control: &mut Producer<Return>) {
        let pushed = to_control.push(Return::Recording {
            id: self.spec.id,
            state: Returned::Ended(why, self.spec),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }

    pub(crate) fn refuse(spec: RecordingSpec, to_control: &mut Producer<Return>) {
        let pushed = to_control.push(Return::Recording {
            id: spec.id,
            state: Returned::Refused(spec),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}
