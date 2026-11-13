//! A voice: one playing of some audio.

use std::sync::Arc;

use super::ControlError;
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd, VoiceId, VoiceReport, VoiceSpec};

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
    pub(crate) fn play(rings: SharedRings, audio: &Audio) -> Result<Self, ControlError> {
        let id = VoiceId::next();
        let report = Arc::new(VoiceReport::new());
        let spec = VoiceSpec {
            id,
            audio: Arc::clone(audio.shared()),
            report: Arc::clone(&report),
        };
        rings.send(VoiceCommand::Add(spec))?;
        Ok(Self { id, rings, report })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.rings.send(VoiceCommand::Remove(self.id))
    }

    pub fn state(&self) -> VoiceState {
        match self.report.ended() {
            Some(why) => VoiceState::Ended(why),
            None => VoiceState::Playing,
        }
    }
}
