//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::collector;
use crate::engine::protocol::recording::{RecordingEnd, RecordingRequest, RecordingState};
use crate::engine::protocol::{Command, Event};
use crate::layout::ChannelLayout;
use crate::request::{ControlEnd, RequestHandle, ReserveError};

/// A refused `record`, holding the audio it was given.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// Stop its voices and try again.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// As many requests as the renderer has room for await their reply;
    /// nothing was sent.
    #[error("too many requests await their reply")]
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
    handle: RequestHandle<(RecordingEnd, Arc<AudioBuffer>), Command, Event>,
    state: RecordingState,
    /// The audio once it comes back, until taken.
    audio: Option<Arc<AudioBuffer>>,
}

impl Recording {
    pub(crate) fn record(
        requests: &ControlEnd<Command, Event>,
        collector: &collector::ControlEnd<Arc<AudioBuffer>>,
        audio: Audio,
    ) -> Result<Self, RecordError> {
        let audio = Self::validate(collector, audio)?;
        let audio = Self::prepare(audio);
        let permit = match requests.try_reserve() {
            Ok(permit) => permit,
            Err(error) => return Err(RecordError::refused(error, Audio::from_shared(audio))),
        };
        let handle = permit.open(RecordingRequest::Start(audio));
        Ok(Self {
            handle,
            state: RecordingState::Recording,
            audio: None,
        })
    }

    pub fn stop(&mut self) {
        self.handle.cancel();
    }

    pub fn state(&mut self) -> RecordingState {
        self.receive();
        self.state
    }

    /// The audio, once the renderer has let go of it: after the recording
    /// ends, or at once if it was refused. `None` until then, and after it
    /// has been taken.
    pub fn take(&mut self) -> Option<Audio> {
        self.receive();
        self.audio.take().map(Audio::from_shared)
    }

    fn validate(
        collector: &collector::ControlEnd<Arc<AudioBuffer>>,
        audio: Audio,
    ) -> Result<Audio, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        // Collected first, so ended voices have let go. The caller gave up
        // theirs, so nothing can clone it: the count can only fall before
        // the send.
        collector.collect();
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

    fn receive(&mut self) {
        if let Ok((why, audio)) = self.handle.try_wait() {
            self.state = RecordingState::Ended(why);
            self.audio = Some(audio);
        }
    }
}

/// Stops a recording still running, since nothing could take its audio
/// once the handle is gone.
impl Drop for Recording {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}
