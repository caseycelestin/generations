//! The recording on the render side, while it runs.

use std::sync::Arc;

use rtrb::Producer;

use crate::engine::protocol::Return;
use crate::engine::protocol::recording::{RecordingEnd, RecordingReport, RecordingState};

#[derive(Debug)]
pub(crate) struct Recording {
    report: Arc<RecordingReport>,
}

impl Recording {
    pub(crate) fn start(report: Arc<RecordingReport>) -> Self {
        report.set(RecordingState::Recording);
        Self { report }
    }

    /// The one way a recording leaves: reports `why`, then hands its report
    /// back to be freed on the control side, never here.
    pub(crate) fn end(self, why: RecordingEnd, to_control: &mut Producer<Return>) {
        self.report.set(RecordingState::Ended(why));
        let pushed = to_control.push(Return::Recording {
            report: self.report,
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }

    /// For a start that never became a recording: the recorder was busy.
    pub(crate) fn refuse(report: Arc<RecordingReport>, to_control: &mut Producer<Return>) {
        report.set(RecordingState::Refused);
        let pushed = to_control.push(Return::Recording { report });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}
