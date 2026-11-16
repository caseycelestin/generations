//! A voice: one playing of some audio.

use std::sync::Arc;

use super::ControlError;
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceRequest, VoiceState};
use crate::request::{ControlEnd, RequestHandle};

#[derive(Debug)]
pub struct Voice {
    handle: RequestHandle<VoiceRequest>,
    state: VoiceState,
}

impl Voice {
    pub(crate) fn play(
        requests: &ControlEnd<VoiceRequest>,
        audio: &Audio,
    ) -> Result<Self, ControlError> {
        let permit = requests.try_reserve()?;
        let audio_clone = Arc::clone(audio.shared());
        let handle = permit.open(VoiceRequest::Play(audio_clone));
        Ok(Self {
            handle,
            state: VoiceState::Playing,
        })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&mut self) {
        self.handle.cancel();
    }

    pub fn state(&mut self) -> VoiceState {
        todo!()
    }
}
