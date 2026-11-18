//! A voice: one playing of some audio.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{ControlError, TryWaitError};
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceData, VoiceState};
use crate::transfer::{ControlEnd, Transfer};

/// Holds a voice slot until `try_wait` returns `Ok` or it is dropped, so
/// one kept after its voice ends must be waited on to give the slot back.
#[derive(Debug)]
pub struct Voice {
    handle: Transfer<VoiceData>,
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(
        transfers: &mut ControlEnd<VoiceData>,
        audio: &Audio,
    ) -> Result<Self, ControlError> {
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        let Ok(handle) = transfers.try_start(data) else {
            return Err(ControlError::Full);
        };
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
