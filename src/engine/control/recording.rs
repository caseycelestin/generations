//! A recording: the main out written into audio the renderer holds.

use std::sync::{Arc, Mutex, PoisonError};

use thiserror::Error;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::HandleId;
use crate::engine::protocol::recording::{RecordingCommand, RecordingSpec, RecordingState};
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
    id: HandleId,
    /// Written by `collect` when the renderer reports this recording.
    state: Arc<Mutex<RecordingState>>,
}

impl Recording {
    pub(crate) fn record(rings: SharedRings, audio: Audio) -> Result<Self, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        // Collected first, so ended voices have let go. The caller gave up
        // theirs, so nothing can clone it: the count can only fall before
        // the send.
        rings.collect();
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        let Ok(mut permit) = rings.reserve() else {
            return Err(RecordError::CommandsFull(audio));
        };
        let id = rings.next_id();
        let mut audio = audio.into_shared();
        let buffer = Arc::get_mut(&mut audio).expect("checked: the only reference");
        buffer.clear();
        buffer.prefault();
        let state = Arc::new(Mutex::new(RecordingState::Pending));
        permit.register(id, Arc::clone(&state));
        permit.send(RecordingCommand::Start(RecordingSpec { id, audio }));
        Ok(Self { rings, id, state })
    }

    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings.send(RecordingCommand::Stop(self.id))
    }

    pub fn state(&self) -> RecordingState {
        self.rings.collect();
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
            self.state(),
            RecordingState::Pending | RecordingState::Recording
        ) {
            let _ = self.stop();
        }
    }
}
