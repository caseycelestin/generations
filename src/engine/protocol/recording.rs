//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;

use super::HandleId;
use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum RecordingCommand {
    Start(RecordingSpec),
    /// A stale id names nothing and does nothing.
    Stop(HandleId),
}

#[derive(Debug)]
pub(crate) struct RecordingSpec {
    pub(crate) id: HandleId,
    pub(crate) audio: Arc<AudioBuffer>,
}

/// A recording's new state. The spec comes back with the states that end
/// it, and only with those.
#[derive(Debug)]
pub(crate) enum Returned {
    Started,
    Refused(RecordingSpec),
    Ended(RecordingEnd, RecordingSpec),
}

/// What the audio thread last said about a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Sent; the renderer has not started it yet.
    Pending,
    Recording,
    /// Never started: the recorder was busy.
    Refused,
    Ended(RecordingEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    /// Its audio has no room left.
    Full,
    /// Its `stop` arrived.
    Stopped,
}
