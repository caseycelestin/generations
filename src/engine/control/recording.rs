//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use super::{StatusError, TryRecvStatusError, TryWaitError};
use crate::audio::{Audio, AudioBuffer};
use crate::engine::protocol::recording::{RecordingEnd, RecordingRequest, RecordingStatus};
use crate::layout::ChannelLayout;
use crate::request::{self, ControlEnd, RequestHandle, ReserveError};

/// A refused `record`, holding the audio it was given.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// Stop its voices and try again.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// Another recording holds the recorder until its reply is taken;
    /// nothing was sent.
    #[error("another recording holds the recorder")]
    Full(Audio),
    /// The renderer is gone; nothing was sent.
    #[error("the renderer is gone")]
    Disconnected(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio)
            | Self::AudioInUse(audio)
            | Self::Full(audio)
            | Self::Disconnected(audio) => audio,
        }
    }

    fn refused(error: ReserveError, audio: Audio) -> Self {
        match error {
            ReserveError::Full => Self::Full(audio),
            ReserveError::Disconnected => Self::Disconnected(audio),
        }
    }
}

#[derive(Debug)]
pub struct Recording {
    handle: RequestHandle<RecordingRequest>,
}

impl Recording {
    pub(crate) fn record(
        requests: &ControlEnd<RecordingRequest>,
        audio: Audio,
    ) -> Result<Self, RecordError> {
        let audio = Self::validate(audio)?;
        let audio = Self::prepare(audio);
        let permit = match requests.try_reserve() {
            Ok(permit) => permit,
            Err(error) => return Err(RecordError::refused(error, Audio::from_shared(audio))),
        };
        let handle = permit.open(RecordingRequest::Start(audio));
        Ok(Self { handle })
    }

    pub fn stop(&mut self) {
        self.handle.cancel();
    }

    /// Asks the renderer, which answers at its next period.
    pub fn request_status(&mut self) -> Result<PendingRecordingStatus, StatusError> {
        let pending = self.handle.request_status()?;
        Ok(PendingRecordingStatus(pending))
    }

    /// How it ended, with the audio holding what was recorded; once.
    pub fn try_wait(&mut self) -> Result<(RecordingEnd, Audio), TryWaitError> {
        let (why, audio) = self.handle.try_wait()?;
        Ok((why, Audio::from_shared(audio)))
    }

    /// Ended voices must have been collected first, or they still count as
    /// holding `audio`.
    fn validate(audio: Audio) -> Result<Audio, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        // The caller gave up theirs, so nothing can clone it: the count can
        // only fall before the send.
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        Ok(audio)
    }

    /// `audio` must have passed `validate`.
    fn prepare(audio: Audio) -> Arc<AudioBuffer> {
        let mut audio = audio.into_shared();
        let buffer = Arc::get_mut(&mut audio).expect("checked: the only reference");
        buffer.clear();
        buffer.prefault();
        audio
    }
}

/// Stops a recording still running, since nothing could take its audio
/// once the handle is gone.
impl Drop for Recording {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}

/// The renderer's answer to one status question about a recording.
#[derive(Debug)]
pub struct PendingRecordingStatus(request::PendingStatus<RecordingRequest>);

impl PendingRecordingStatus {
    pub fn try_recv(&mut self) -> Result<RecordingStatus, TryRecvStatusError> {
        let status = self.0.try_recv()?;
        Ok(status)
    }
}
