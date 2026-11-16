//! The control side: runs on UI or event threads. May lock and allocate.

mod recording;
mod requests;
mod voice;

pub use recording::{PendingRecordingStatus, RecordError, Recording};
pub(crate) use requests::Requests;
pub use requests::{StatusError, TryRecvStatusError, TryWaitError};
pub use voice::{PendingVoiceStatus, PlayError, Voice};

use std::sync::Arc;

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::duplex;
use crate::engine::protocol::voice::VoiceRequest;
use crate::engine::protocol::{BufferAddress, Command, Event};
use crate::{collector, request};

/// Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StopError {
    /// They clear within a period.
    #[error("earlier stops are still on their way")]
    Busy,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<request::ReserveError> for StopError {
    fn from(error: request::ReserveError) -> Self {
        match error {
            request::ReserveError::Full => Self::Busy,
            request::ReserveError::Disconnected => Self::RendererGone,
        }
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

    pub fn play(&self, audio: &Audio) -> Result<Voice, PlayError> {
        self.collector.collect();
        Voice::play(&self.requests.voices, audio)
    }

    /// Stops every voice playing `audio`.
    pub fn stop(&self, audio: &Audio) -> Result<(), StopError> {
        let address = BufferAddress::of(audio.shared());
        let permit = self.requests.voices.try_reserve_including_headroom()?;
        // Its handle is dropped: the reply says only that it was applied.
        permit.open(VoiceRequest::StopAudio(address));
        Ok(())
    }

    /// Stops every voice; a recording runs on.
    pub fn stop_all(&self) -> Result<(), StopError> {
        let permit = self.requests.voices.try_reserve_including_headroom()?;
        permit.open(VoiceRequest::StopAll);
        Ok(())
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
