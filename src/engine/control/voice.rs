//! A voice: one playing of some audio.

use std::sync::Arc;

use super::ControlError;
use crate::audio::Audio;
use crate::duplex::{ControlEnd, Handle};
use crate::engine::protocol::voice::{VoiceCommand, VoiceState};
use crate::engine::protocol::{Command, Event};

#[derive(Debug)]
pub struct Voice {
    handle: Handle<Command, Event>,
    state: VoiceState,
}

impl Voice {
    /// Reserves one event, its end: a voice says nothing when it starts.
    pub(crate) fn play(
        channel: &ControlEnd<Command, Event>,
        audio: &Audio,
    ) -> Result<Self, ControlError> {
        let permit = channel.try_reserve()?;
        let handle = permit.open(VoiceCommand::Add(Arc::clone(audio.shared())));
        Ok(Self {
            handle,
            state: VoiceState::Playing,
        })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.handle
            .send(VoiceCommand::Remove)
            .map_err(ControlError::from)
    }

    pub fn state(&mut self) -> VoiceState {
        while let Some(event) = self.handle.try_recv() {
            let Event::Voice(end) = event else {
                unreachable!("a handle hears only its own feature's events");
            };
            self.state = VoiceState::Ended(end);
        }
        self.state
    }
}
