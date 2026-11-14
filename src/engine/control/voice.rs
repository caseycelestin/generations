//! A voice: one playing of some audio.

use std::sync::Arc;

use super::ControlError;
use super::handle::{Handle, Inbox};
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::Event;
use crate::engine::protocol::voice::{VoiceCommand, VoiceEvent, VoiceSpec, VoiceState};

#[derive(Debug)]
pub struct Voice {
    handle: Handle<VoiceEvent>,
}

impl Voice {
    pub(crate) fn play(rings: SharedRings, audio: &Audio) -> Result<Self, ControlError> {
        let spec = VoiceSpec {
            audio: Arc::clone(audio.shared()),
        };
        let Ok(handle) = Handle::open(&rings, spec, VoiceCommand::Add) else {
            return Err(ControlError::CommandsFull);
        };
        Ok(Self { handle })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.handle.send(VoiceCommand::Remove)
    }

    pub fn state(&self) -> VoiceState {
        self.handle.inbox().state
    }
}

/// Keeps only the state. Its end is the last event.
impl Inbox for VoiceEvent {
    fn deliver(&mut self, event: Event) -> bool {
        let Event::Voice(event) = event else {
            unreachable!("an id belongs to one feature");
        };
        self.state = event.state;
        matches!(event.state, VoiceState::Ended(_))
    }
}
