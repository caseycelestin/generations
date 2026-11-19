//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::engine::protocol::recording::{RecordingData, RecordingState};
use crate::layout::ChannelLayout;
use crate::transfer;

/// Nothing was recorded; the audio comes back in it.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// A voice holds its audio until it ends; stop it, or wait for it to
    /// end.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// A stopped recording gives its place back at the renderer's next
    /// period.
    #[error("another recording is running")]
    Full(Audio),
    #[error("the renderer is gone")]
    RendererGone(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio)
            | Self::AudioInUse(audio)
            | Self::Full(audio)
            | Self::RendererGone(audio) => audio,
        }
    }
}

impl From<transfer::StartError<RecordingData>> for RecordError {
    fn from(error: transfer::StartError<RecordingData>) -> Self {
        match error {
            transfer::StartError::Full(data) => Self::Full(Audio::from_shared(data.audio)),
            transfer::StartError::Disconnected(data) => {
                Self::RendererGone(Audio::from_shared(data.audio))
            }
        }
    }
}

/// The audio was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryTakeError {
    /// Still recording, or its audio is on the way back.
    #[error("still recording")]
    Recording,
    #[error("the audio was already taken")]
    Taken,
    /// Not expected: a dropped renderer hands its recording back.
    #[error("the renderer is gone, and the audio with it")]
    RendererGone,
}

impl From<transfer::TryWaitError> for TryTakeError {
    fn from(error: transfer::TryWaitError) -> Self {
        match error {
            transfer::TryWaitError::Empty => Self::Recording,
            transfer::TryWaitError::Taken => Self::Taken,
            transfer::TryWaitError::Disconnected => Self::RendererGone,
        }
    }
}

/// Holds the recorder until the recording ends.
#[derive(Debug)]
pub struct Recording {
    transfer: transfer::Transfer<RecordingData>,
    state: Arc<RecordingState>,
}

impl Recording {
    pub(crate) fn record(
        transfers: &mut transfer::ControlEnd<RecordingData>,
        audio: Audio,
    ) -> Result<Self, RecordError> {
        let audio = Self::validate(audio)?;
        let audio = Self::prepare(audio);
        let state = Arc::new(RecordingState::default());
        let data = RecordingData {
            audio,
            state: Arc::clone(&state),
        };
        let transfer = transfers.try_start(data)?;
        Ok(Self { transfer, state })
    }

    pub fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }

    /// Tries to take the audio out, once it is back.
    pub fn try_take(&mut self) -> Result<Audio, TryTakeError> {
        let data = self.transfer.try_wait()?;
        Ok(Audio::from_shared(data.audio))
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
        self.stop();
    }
}
