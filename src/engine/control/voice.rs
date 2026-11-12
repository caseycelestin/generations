//! A voice: one playing of a sample.

use std::sync::Arc;

use super::{Control, ControlError};
use crate::engine::protocol::{Command, VoiceCommand, VoiceId};
use crate::project::sample::Sample;

#[derive(Debug)]
pub struct Voice {
    id: VoiceId,
    control: Control,
}

impl Voice {
    /// Starts `sample` at the renderer's next period.
    pub fn play(control: &Control, sample: &Sample) -> Result<Self, ControlError> {
        let id = VoiceId::next();
        control.send(Command::Voice(VoiceCommand::Add {
            id,
            audio: Arc::clone(sample.audio()),
        }))?;
        Ok(Self {
            id,
            control: control.clone(),
        })
    }

    /// Stops the voice at the renderer's next period. A dropped voice
    /// plays out instead.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.control
            .send(Command::Voice(VoiceCommand::Remove(self.id)))
    }
}
