//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::recording::{
    RecordingCommand, RecordingReport, RecordingSpec, RecordingState,
};

#[derive(Debug)]
pub struct Recording {
    rings: SharedRings,
    report: Arc<RecordingReport>,
}

impl Recording {
    pub(crate) fn record(rings: SharedRings, audio: Audio) -> Result<Self, ControlError> {
        let report = Arc::new(RecordingReport::new());
        let spec = RecordingSpec {
            audio: Arc::clone(audio.shared()),
            report: Arc::clone(&report),
        };
        rings.send(RecordingCommand::Start(spec))?;
        Ok(Self { rings, report })
    }

    /// Stops the recording at the renderer's next period.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings.send(RecordingCommand::Stop)
    }

    /// What the audio thread has said, as of now.
    pub fn state(&self) -> RecordingState {
        self.report.state()
    }

    /// The recorded audio, once the recording has ended. Before then the
    /// handle comes back, so it can be asked again.
    pub fn take(self) -> Result<Audio, Self> {
        todo!("hand back the audio once it has come back")
    }
}
