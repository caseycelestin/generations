//! A voice: one playing of some audio.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use thiserror::Error;

use super::TryWaitError;
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceData, VoiceState};
use crate::request::{ControlEnd, ReserveError};
use crate::transfer::Transfer;

/// Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PlayError {
    /// Wait on or drop voices that have ended, then try again.
    #[error("every voice slot is held")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<ReserveError> for PlayError {
    fn from(error: ReserveError) -> Self {
        match error {
            ReserveError::Full => Self::Full,
            ReserveError::Disconnected => Self::RendererGone,
        }
    }
}

/// Holds a voice slot until `try_wait` returns `Ok` or it is dropped, so
/// one kept after its voice ends must be waited on to give the slot back.
#[derive(Debug)]
pub struct Voice {
    handle: Transfer<VoiceData>,
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(requests: &ControlEnd<VoiceData>, audio: &Audio) -> Result<Self, PlayError> {
        let permit = requests.try_reserve()?;
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        let handle = permit.open(data);
        Ok(Self { handle, state })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }

    /// `Ok` once it has ended.
    pub fn try_wait(&mut self) -> Result<(), TryWaitError> {
        self.handle.try_wait()?;
        Ok(())
    }
}
