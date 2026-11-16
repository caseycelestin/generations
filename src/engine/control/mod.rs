//! The control side: runs on UI or event threads. May lock and allocate.

mod recording;
mod requests;
mod voice;

pub use recording::{PendingRecordingStatus, RecordError, Recording};
pub(crate) use requests::Requests;
pub use requests::{StatusError, TryRecvStatusError, TryWaitError};
pub use voice::Voice;

use std::sync::Arc;

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::duplex::{self, Full, ReserveError};
use crate::engine::protocol::voice::VoiceRequest;
use crate::engine::protocol::{BufferAddress, Command, Event};
use crate::{collector, request};

#[derive(Debug, Error)]
pub enum ControlError {
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull,
    /// Voices and a recording not yet ended hold every event slot; nothing
    /// was sent.
    #[error("every event slot is held")]
    EventsFull,
    /// As many requests as the renderer has room for await their reply;
    /// nothing was sent.
    #[error("too many requests await their reply")]
    Full,
    /// The renderer is gone; nothing was sent.
    #[error("the renderer is gone")]
    Disconnected,
}

impl From<request::ReserveError> for ControlError {
    fn from(error: request::ReserveError) -> Self {
        match error {
            request::ReserveError::Full => Self::Full,
            request::ReserveError::Disconnected => Self::Disconnected,
        }
    }
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
    channel: duplex::ControlEnd<Command, Event>,
    /// Collected before every play and record, which its size assumes.
    collector: collector::ControlEnd<Arc<AudioBuffer>>,
    requests: Requests,
}

impl Control {
    pub(crate) fn new(
        channel: duplex::ControlEnd<Command, Event>,
        collector: collector::ControlEnd<Arc<AudioBuffer>>,
        requests: Requests,
    ) -> Self {
        Self {
            channel,
            collector,
            requests,
        }
    }

    pub fn play(&self, audio: &Audio) -> Result<Voice, ControlError> {
        self.collector.collect();
        Voice::play(&self.requests, audio)
    }

    /// Stops every voice playing `audio`.
    pub fn stop(&self, audio: &Audio) -> Result<(), ControlError> {
        let address = BufferAddress::of(audio.shared());
        self.channel
            .try_send(VoiceRequest::StopAudio(address))
            .map_err(ControlError::from)
    }

    /// Stops every voice; a recording runs on.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.channel
            .try_send(VoiceRequest::StopAll)
            .map_err(ControlError::from)
    }

    /// Records the main out into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs, or while a voice holds `audio`.
    pub fn record(&self, audio: Audio) -> Result<Recording, RecordError> {
        self.collector.collect();
        Recording::record(&self.requests.recording, audio)
    }

    /// Frees what the renderer has let go of, returning it to
    /// `AudioMemory`. Every play and record does this first; call it when
    /// the app goes quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.collector.collect();
    }
}
