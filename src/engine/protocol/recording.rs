//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum RecordingCommand {
    Start(RecordingSpec),
    /// Stops the sender's take; from any other sender, does nothing.
    Stop,
}

#[derive(Debug)]
pub(crate) struct RecordingEvent {
    pub(crate) state: RecordingState,
    /// Back only when the renderer lets go: on `Refused` and `Ended`.
    pub(crate) spec: Option<RecordingSpec>,
}

#[derive(Debug)]
pub(crate) struct RecordingSpec {
    pub(crate) audio: Arc<AudioBuffer>,
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
