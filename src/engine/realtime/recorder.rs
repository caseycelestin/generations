//! The renderer's one recorder: it writes at most one recording at a time.

use super::recording::Recording;
use crate::engine::protocol::recording::RecordingData;
use crate::transfer;

#[derive(Debug)]
pub(crate) struct Recorder {
    recording: Option<Recording>,
    transfers: transfer::RealtimeEnd<RecordingData>,
}

impl Recorder {
    pub(crate) fn new(transfers: transfer::RealtimeEnd<RecordingData>) -> Self {
        Self {
            recording: None,
            transfers,
        }
    }
}
