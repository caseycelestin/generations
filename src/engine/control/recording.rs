//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::recording::{RecordingCommand, RecordingEnd, RecordingReport};

#[derive(Debug)]
pub struct Recording {
    rings: SharedRings,
    report: Arc<RecordingReport>,
}

/// What the audio thread last said about a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    /// Not reported ended yet: recording, or about to.
    Recording,
    Ended(RecordingEnd),
}

impl Recording {
    pub(crate) fn record(rings: SharedRings, _audio: Audio) -> Result<Self, ControlError> {
        let report = Arc::new(RecordingReport::new());
        rings.send(RecordingCommand::Start(Arc::clone(&report)))?;
        Ok(Self { rings, report })
    }

    /// Stops the recording at the renderer's next period.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings.send(RecordingCommand::Stop)
    }

    /// What the audio thread has said, as of now.
    pub fn state(&self) -> RecordingState {
        match self.report.ended() {
            Some(why) => RecordingState::Ended(why),
            None => RecordingState::Recording,
        }
    }

    /// The recorded audio, once the recording has ended. Before then the
    /// handle comes back, so it can be asked again.
    pub fn take(self) -> Result<Audio, Self> {
        todo!("hand back the audio once it has come back")
    }
}
