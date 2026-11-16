//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::request::Request;

#[derive(Debug)]
pub(crate) enum RecordingRequest {
    Start(Arc<AudioBuffer>),
}

/// Its audio comes back with why it ended, which says what the audio holds.
impl Request for RecordingRequest {
    type Reply = (RecordingEnd, Arc<AudioBuffer>);
    type Status = RecordingStatus;
}

/// The renderer's answer, while it holds the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingStatus {
    Recording,
    Ended(RecordingEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    /// Its audio has no room left.
    Full,
    /// Its `stop` arrived.
    Stopped,
}
