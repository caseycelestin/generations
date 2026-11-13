//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::recording::{
    RecordingCommand, RecordingReport, RecordingSpec, RecordingState,
};

/// A refused `record`, holding the audio it was given.
#[derive(Debug, Error)]
pub enum RecordError {
    /// A voice still holds the audio, so it cannot be written. Stop its
    /// voices and try again.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::AudioInUse(audio) | Self::CommandsFull(audio) => audio,
        }
    }
}

#[derive(Debug)]
pub struct Recording {
    rings: SharedRings,
    report: Arc<RecordingReport>,
}

impl Recording {
    pub(crate) fn record(rings: SharedRings, audio: Audio) -> Result<Self, RecordError> {
        // Only this side clones or drops the audio, and the caller gave up
        // theirs, so after a collect the count cannot rise before the send.
        rings.collect();
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        let report = Arc::new(RecordingReport::new());
        let spec = RecordingSpec {
            audio: Arc::clone(audio.shared()),
            report: Arc::clone(&report),
        };
        rings
            .send(RecordingCommand::Start(spec))
            .map_err(|_| RecordError::CommandsFull(audio))?;
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
