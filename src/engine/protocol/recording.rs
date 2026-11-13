//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::BufferAddress;
use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum RecordingCommand {
    Start(RecordingSpec),
    /// Stop the recording into this audio, if it is still running.
    Stop(BufferAddress),
}

/// A recording as asked for: what to record into and where to report.
#[derive(Debug)]
pub(crate) struct RecordingSpec {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) report: Arc<RecordingReport>,
}

/// The audio thread's report on a recording: its latest state, which
/// only moves forward. Starts `Pending`; after that only the renderer writes.
#[derive(Debug)]
pub(crate) struct RecordingReport(AtomicU8);

impl RecordingReport {
    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(Self::code(RecordingState::Pending)))
    }

    /// For the renderer, at each change.
    pub(crate) fn set(&self, state: RecordingState) {
        self.0.store(Self::code(state), Ordering::Release);
    }

    pub(crate) fn state(&self) -> RecordingState {
        match self.0.load(Ordering::Acquire) {
            1 => RecordingState::Recording,
            2 => RecordingState::Refused,
            3 => RecordingState::Ended(RecordingEnd::Full),
            4 => RecordingState::Ended(RecordingEnd::Stopped),
            _ => RecordingState::Pending,
        }
    }

    fn code(state: RecordingState) -> u8 {
        match state {
            RecordingState::Pending => 0,
            RecordingState::Recording => 1,
            RecordingState::Refused => 2,
            RecordingState::Ended(RecordingEnd::Full) => 3,
            RecordingState::Ended(RecordingEnd::Stopped) => 4,
        }
    }
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

/// Why a recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    /// Its audio has no room left.
    Full,
    /// Its `stop` arrived.
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_report_is_pending() {
        assert_eq!(RecordingReport::new().state(), RecordingState::Pending);
    }

    #[test]
    fn each_state_reads_back_as_itself() {
        for state in [
            RecordingState::Pending,
            RecordingState::Recording,
            RecordingState::Refused,
            RecordingState::Ended(RecordingEnd::Full),
            RecordingState::Ended(RecordingEnd::Stopped),
        ] {
            let report = RecordingReport::new();
            report.set(state);
            assert_eq!(report.state(), state);
        }
    }
}
