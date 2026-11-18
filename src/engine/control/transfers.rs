//! The engine's face on the transfer channels, in the engine's terms.

use super::TryWaitError;
use crate::engine::protocol::inputs::InputData;
use crate::engine::protocol::voice::VoiceData;
use crate::transfer;

impl From<transfer::TryWaitError> for TryWaitError {
    fn from(error: transfer::TryWaitError) -> Self {
        match error {
            transfer::TryWaitError::Empty => Self::NotEnded,
            transfer::TryWaitError::Taken => Self::Taken,
            transfer::TryWaitError::Disconnected => Self::RendererGone,
        }
    }
}

/// One transfer channel per actor in the renderer, as each moves off its
/// request channel.
#[derive(Debug)]
pub(crate) struct Transfers {
    pub(crate) voices: transfer::ControlEnd<VoiceData>,
    pub(crate) inputs: transfer::ControlEnd<InputData>,
}

impl Transfers {
    pub(crate) fn take_in(&mut self) {
        self.voices.take_in();
        self.inputs.take_in();
    }
}
