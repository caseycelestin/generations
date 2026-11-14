//! A voice: one playing of some audio.

use std::sync::Arc;

use super::ControlError;
use super::handle::{Handle, Inbox};
use super::rings::SharedRings;
use crate::audio::Audio;
use crate::engine::protocol::Event;
use crate::engine::protocol::voice::{VoiceCommand, VoiceEvent, VoiceId, VoiceSpec, VoiceState};

#[derive(Debug)]
pub struct Voice {
    /// Names the voice in `Remove` until voices are named by handle id.
    id: VoiceId,
    handle: Handle<VoiceEvent>,
}

impl Voice {
    pub(crate) fn play(rings: SharedRings, audio: &Audio) -> Result<Self, ControlError> {
        let id = VoiceId::next();
        let spec = VoiceSpec {
            id,
            audio: Arc::clone(audio.shared()),
        };
        let Ok(handle) = Handle::open(&rings, spec, VoiceCommand::Add) else {
            return Err(ControlError::CommandsFull);
        };
        Ok(Self { id, handle })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.handle.send(VoiceCommand::Remove(self.id))
    }

    pub fn state(&self) -> VoiceState {
        self.handle.inbox().state
    }
}

/// Keeps only the state: nothing takes a voice's audio back, so its spec
/// is dropped here, on the control side. The event that hands it back is
/// the last.
impl Inbox for VoiceEvent {
    fn deliver(&mut self, event: Event) -> bool {
        let Event::Voice(event) = event else {
            unreachable!("an id belongs to one feature");
        };
        self.state = event.state;
        event.spec.is_some()
    }
}
