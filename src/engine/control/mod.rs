//! The control side: runs on UI or event threads. May lock and allocate.

mod recording;
mod voice;

pub use recording::{RecordError, Recording};
pub use voice::Voice;

use std::sync::Arc;

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::collector::Collector;
use crate::duplex::{ControlEnd, Full, ReserveError};
use crate::engine::protocol::voice::VoiceCommand;
use crate::engine::protocol::{BufferAddress, Command, Event};

#[derive(Debug, Error)]
pub enum ControlError {
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull,
    /// Voices and a recording not yet ended hold every event slot; nothing
    /// was sent.
    #[error("every event slot is held")]
    EventsFull,
}

impl From<ReserveError> for ControlError {
    fn from(error: ReserveError) -> Self {
        match error {
            ReserveError::CommandsFull => Self::CommandsFull,
            ReserveError::EventsFull => Self::EventsFull,
        }
    }
}

/// The refused command is dropped here, on the control side.
impl From<Full<Command>> for ControlError {
    fn from(_: Full<Command>) -> Self {
        Self::CommandsFull
    }
}

/// Clones act on the same renderer. Every command, from here or from a
/// handle, takes effect at the renderer's next period.
#[derive(Debug, Clone)]
pub struct Control {
    channel: ControlEnd<Command, Event>,
    /// Collected before every play and record, which its size assumes.
    collector: Collector<Arc<AudioBuffer>>,
}

impl Control {
    pub(crate) fn new(
        channel: ControlEnd<Command, Event>,
        collector: Collector<Arc<AudioBuffer>>,
    ) -> Self {
        Self { channel, collector }
    }

    pub fn play(&self, audio: &Audio) -> Result<Voice, ControlError> {
        self.collector.collect();
        Voice::play(&self.channel, audio)
    }

    /// Stops every voice playing `audio`.
    pub fn stop(&self, audio: &Audio) -> Result<(), ControlError> {
        let address = BufferAddress::of(audio.shared());
        self.channel
            .try_send(VoiceCommand::RemoveAudio(address))
            .map_err(ControlError::from)
    }

    /// Stops every voice; a recording runs on.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.channel
            .try_send(VoiceCommand::RemoveAll)
            .map_err(ControlError::from)
    }

    /// Records the main out into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs, as its state shows, and at once while a voice holds
    /// `audio`.
    pub fn record(&self, audio: Audio) -> Result<Recording, RecordError> {
        Recording::record(&self.channel, &self.collector, audio)
    }

    /// Frees what the renderer has let go of, returning it to
    /// `AudioMemory`. Every play and record does this first; call it when
    /// the app goes quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.collector.collect();
    }
}
