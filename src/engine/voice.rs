//! A voice: one playing of some audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::transfer::{self, ControlEnd};

/// Shared by both sides while the voice plays.
#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    pub(crate) stop: AtomicBool,
}

/// Comes back whole once the voice ends, so its audio and state are freed
/// on the control side.
#[derive(Debug)]
pub(crate) struct VoiceData {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) state: Arc<VoiceState>,
}

/// Nothing was played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VoiceError {
    /// Stop one, or wait for one to end, then try again.
    #[error("every voice is held")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<transfer::StartError<VoiceData>> for VoiceError {
    fn from(error: transfer::StartError<VoiceData>) -> Self {
        match error {
            transfer::StartError::Full(_) => Self::Full,
            transfer::StartError::Disconnected(_) => Self::RendererGone,
        }
    }
}

/// Its voice holds a place until it ends, whether the handle is kept or
/// dropped.
#[derive(Debug)]
pub struct Voice {
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(
        transfers: &mut ControlEnd<VoiceData>,
        audio: &Audio,
    ) -> Result<Self, VoiceError> {
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        transfers.try_start(data)?;
        Ok(Self { state })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}
