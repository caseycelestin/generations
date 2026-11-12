//! A voice: one playing of a sample.

use std::sync::Arc;

use super::ControlError;
use super::rings::SharedRings;
use crate::engine::protocol::{Command, VoiceCommand, VoiceId};
use crate::project::sample::Sample;

#[derive(Debug)]
pub struct Voice {
    id: VoiceId,
    rings: SharedRings,
}

impl Voice {
    pub(crate) fn play(rings: SharedRings, sample: &Sample) -> Result<Self, ControlError> {
        let id = VoiceId::next();
        rings.send(Command::Voice(VoiceCommand::Add {
            id,
            audio: Arc::clone(sample.audio()),
        }))?;
        Ok(Self { id, rings })
    }

    /// Stops the voice at the renderer's next period. A dropped voice
    /// plays out instead.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings
            .send(Command::Voice(VoiceCommand::Remove(self.id)))
    }
}
