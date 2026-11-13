//! The control side: runs on UI or event threads. May lock and allocate.

mod rings;
mod voice;

pub use voice::{Voice, VoiceState};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::audio::Audio;
use crate::engine::protocol::voice::{BufferAddress, VoiceCommand};
use crate::engine::protocol::{Command, Return};
use rings::SharedRings;

#[derive(Debug, Error)]
pub enum ControlError {
    /// The renderer has not caught up; the command was dropped.
    #[error("the command ring is full")]
    CommandsFull,
}

/// One renderer, for the control side: makes the handles that act on it.
/// Clones are the same renderer.
#[derive(Debug, Clone)]
pub struct Control {
    rings: SharedRings,
}

impl Control {
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self {
            rings: SharedRings::new(to_renderer, from_renderer),
        }
    }

    /// Starts `audio` at the renderer's next period.
    pub fn play(&self, audio: &Audio) -> Result<Voice, ControlError> {
        Voice::play(self.rings.clone(), audio)
    }

    /// Stops every voice playing `audio`, at the renderer's next period.
    pub fn stop(&self, audio: &Audio) -> Result<(), ControlError> {
        let address = BufferAddress::of(audio.shared());
        self.rings.send(VoiceCommand::RemoveAudio(address))
    }

    /// Stops every voice, at the renderer's next period.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.rings.send(VoiceCommand::RemoveAll)
    }

    /// Frees what the renderer has handed back, returning it to
    /// `AudioMemory`. Every send does this first; call it when the app goes
    /// quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.rings.collect();
    }
}
