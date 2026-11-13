//! The control side: runs on UI or event threads. May lock and allocate.

mod recording;
mod rings;
mod voice;

pub use recording::{RecordError, Recording};
pub use voice::Voice;

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::audio::Audio;
use crate::engine::protocol::voice::VoiceCommand;
use crate::engine::protocol::{BufferAddress, Command, Event, HandleId, Message};
use rings::SharedRings;

#[derive(Debug, Error)]
pub enum ControlError {
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull,
}

/// Clones act on the same renderer. Every command, from here or from a
/// handle, takes effect at the renderer's next period.
#[derive(Debug, Clone)]
pub struct Control {
    rings: SharedRings,
    id: HandleId,
}

impl Control {
    pub(crate) fn new(
        to_renderer: Producer<Message<Command>>,
        from_renderer: Consumer<Message<Event>>,
    ) -> Self {
        let rings = SharedRings::new(to_renderer, from_renderer);
        let id = rings.next_id();
        Self { rings, id }
    }

    pub fn play(&self, audio: &Audio) -> Result<Voice, ControlError> {
        Voice::play(self.rings.clone(), audio)
    }

    /// Stops every voice playing `audio`.
    pub fn stop(&self, audio: &Audio) -> Result<(), ControlError> {
        let address = BufferAddress::of(audio.shared());
        self.rings.send(self.id, VoiceCommand::RemoveAudio(address))
    }

    /// Stops every voice; a recording runs on.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.rings.send(self.id, VoiceCommand::RemoveAll)
    }

    /// Records the main out into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs, as its state shows, and at once while a voice holds
    /// `audio`.
    pub fn record(&self, audio: Audio) -> Result<Recording, RecordError> {
        Recording::record(self.rings.clone(), audio)
    }

    /// Frees what the renderer has handed back, returning it to
    /// `AudioMemory`. Every send does this first; call it when the app goes
    /// quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.rings.collect();
    }
}
