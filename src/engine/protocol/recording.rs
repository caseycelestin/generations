//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum RecordingCommand {
    Start(Arc<AudioBuffer>),
    /// Stops the sender's take; from any other sender, does nothing.
    Stop,
}

/// The audio comes back with the last event, and only then.
#[derive(Debug)]
pub(crate) enum RecordingEvent {
    Started,
    Refused(Arc<AudioBuffer>),
    Ended(RecordingEnd, Arc<AudioBuffer>),
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
