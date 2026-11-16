//! A voice: one playing of some audio.

use std::sync::Arc;

use super::{ControlError, StatusError, TryRecvStatusError, TryWaitError};
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceRequest, VoiceStatus};
use crate::request::{self, ControlEnd, RequestHandle};

/// Holds a voice slot until `try_wait` returns `Ok` or it is dropped, so
/// one kept after its voice ends must be waited on to give the slot back.
#[derive(Debug)]
pub struct Voice {
    handle: RequestHandle<VoiceRequest>,
}

impl Voice {
    pub(crate) fn play(
        requests: &ControlEnd<VoiceRequest>,
        audio: &Audio,
    ) -> Result<Self, ControlError> {
        let permit = requests.try_reserve()?;
        let audio_clone = Arc::clone(audio.shared());
        let handle = permit.open(VoiceRequest::Play(audio_clone));
        Ok(Self { handle })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&mut self) {
        self.handle.cancel();
    }

    /// Asks the renderer, which answers at its next period.
    pub fn request_status(&mut self) -> Result<PendingVoiceStatus, StatusError> {
        let pending = self.handle.request_status()?;
        Ok(PendingVoiceStatus(pending))
    }

    /// `Ok` once it has ended; taking that also frees its slot.
    pub fn try_wait(&mut self) -> Result<(), TryWaitError> {
        self.handle.try_wait()?;
        Ok(())
    }
}

/// The renderer's answer to one status question about a voice.
#[derive(Debug)]
pub struct PendingVoiceStatus(request::PendingStatus<VoiceRequest>);

impl PendingVoiceStatus {
    pub fn try_recv(&mut self) -> Result<VoiceStatus, TryRecvStatusError> {
        let status = self.0.try_recv()?;
        Ok(status)
    }
}
