//! A voice: one playing of a sample.

use std::sync::Arc;

use super::ControlError;
use super::rings::SharedRings;
use crate::engine::protocol::Command;
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd, VoiceId, VoiceReport};
use crate::project::sample::Sample;

#[derive(Debug)]
pub struct Voice {
    id: VoiceId,
    rings: SharedRings,
    report: Arc<VoiceReport>,
}

/// What the audio thread last said about a voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Not reported ended yet: playing, or about to.
    Playing,
    Ended(VoiceEnd),
}

impl Voice {
    pub(crate) fn play(rings: SharedRings, sample: &Sample) -> Result<Self, ControlError> {
        let id = VoiceId::next();
        let report = Arc::new(VoiceReport::new());
        rings.send(Command::Voice(VoiceCommand::Add {
            id,
            audio: Arc::clone(sample.audio()),
            report: Arc::clone(&report),
        }))?;
        Ok(Self { id, rings, report })
    }

    /// Stops the voice at the renderer's next period. A dropped voice
    /// plays out instead.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings
            .send(Command::Voice(VoiceCommand::Remove(self.id)))
    }

    /// What the audio thread has said, as of now.
    pub fn state(&self) -> VoiceState {
        match self.report.ended() {
            Some(why) => VoiceState::Ended(why),
            None => VoiceState::Playing,
        }
    }
}
