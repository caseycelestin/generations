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
}

/// What the audio thread last said about a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Not reported ended yet: recording, or about to.
    Recording,
    Ended(RecordingEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    /// Never started: the recorder was busy, so its audio is empty.
    Refused,
    /// Its audio has no room left.
    Full,
    /// Its `stop` arrived.
    Stopped,
}
