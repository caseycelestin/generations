//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::BufferAddress;
use crate::engine::protocol::recording::{
    RecordingCommand, RecordingReport, RecordingSpec, RecordingState,
};
use crate::layout::ChannelLayout;

/// A refused `record`, holding the audio it was given.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// Stop its voices and try again.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio) | Self::AudioInUse(audio) | Self::CommandsFull(audio) => audio,
        }
    }
}

#[derive(Debug)]
pub struct Recording {
    rings: SharedRings,
    address: BufferAddress,
    report: Arc<RecordingReport>,
}

impl Recording {
    pub(crate) fn record(rings: SharedRings, audio: Audio) -> Result<Self, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        let Ok(permit) = rings.reserve() else {
            return Err(RecordError::CommandsFull(audio));
        };
        // Only this side clones or drops the audio, the caller gave up
        // theirs, and the permit has just collected under the lock, so the
        // count cannot rise before the send.
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        let mut audio = audio.into_shared();
        let buffer = Arc::get_mut(&mut audio).expect("checked: the only reference");
        buffer.clear();
        buffer.prefault();
        let address = BufferAddress::of(&audio);
        let report = Arc::new(RecordingReport::new());
        permit.send(RecordingCommand::Start(RecordingSpec {
            audio,
            report: Arc::clone(&report),
        }));
        Ok(Self {
            rings,
            address,
            report,
        })
    }

    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings.send(RecordingCommand::Stop(self.address))
    }

    pub fn state(&self) -> RecordingState {
        self.report.state()
    }

    /// Until the recording has ended the handle comes back, to ask again.
    pub fn take(self) -> Result<Audio, Self> {
        todo!("hand back the audio once it has come back")
    }
}

/// Stops a recording still running, since nothing could take its audio
/// once the handle is gone. A full ring loses the stop; the take then runs
/// until its audio is full.
impl Drop for Recording {
    fn drop(&mut self) {
        if matches!(
            self.report.state(),
            RecordingState::Pending | RecordingState::Recording
        ) {
            let _ = self.stop();
        }
    }
}
