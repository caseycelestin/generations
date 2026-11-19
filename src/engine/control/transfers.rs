//! The engine's face on the transfer channels, in the engine's terms.

use crate::engine::protocol::inputs::InputData;
use crate::engine::protocol::recording::RecordingData;
use crate::engine::protocol::voice::VoiceData;
use crate::transfer;

/// One transfer channel per actor in the renderer.
#[derive(Debug)]
pub(crate) struct Transfers {
    pub(crate) voices: transfer::ControlEnd<VoiceData>,
    pub(crate) inputs: transfer::ControlEnd<InputData>,
    pub(crate) recordings: transfer::ControlEnd<RecordingData>,
}

impl Transfers {
    pub(crate) fn take_in(&mut self) {
        self.voices.take_in();
        self.inputs.take_in();
        self.recordings.take_in();
    }
}
