//! The control side: runs on UI or event threads. May lock and allocate.

mod input;
mod recording;
mod requests;
mod voice;

pub use input::{Input, PendingInputStatus};
pub use recording::{PendingRecordingStatus, RecordError, Recording};
pub(crate) use requests::Requests;
pub use requests::{ControlError, StatusError, TryRecvStatusError, TryWaitError};
pub use voice::{PendingVoiceStatus, PlayError, Voice};

use std::sync::Arc;

use thiserror::Error;

use crate::audio::Audio;
use crate::engine::protocol::inputs::InputChannels;
use crate::engine::protocol::voice::VoiceRequest;
use crate::request;

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

/// Clones act on the same renderer. Every request, from here or from a
/// handle, takes effect at the renderer's next period.
#[derive(Debug, Clone)]
pub struct Control {
    requests: Requests,
    /// The capture device's, or zero without one.
    capture_channels: usize,
}

impl Control {
    pub(crate) fn new(requests: Requests, capture_channels: usize) -> Self {
        Self {
            requests,
            capture_channels,
        }
    }

    pub fn play(&self, audio: &Audio) -> Result<Voice, PlayError> {
        Voice::play(&self.requests.voices, audio)
    }

    /// Stops every voice playing `audio`.
    pub fn stop(&self, audio: &Audio) -> Result<(), StopError> {
        let permit = self.requests.voices.try_reserve_including_headroom()?;
        let audio_clone = Arc::clone(audio.shared());
        // Its handle is dropped: the reply only brings the clone back.
        permit.open(VoiceRequest::StopAudio(audio_clone));
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
        self.collect();
        Recording::record(&self.requests.recording, audio)
    }

    /// Inputs may read the same channels; each gets its own copy.
    ///
    /// ```no_run
    /// use std::thread;
    ///
    /// use generations::engine::{self, Capture, InputChannels};
    ///
    /// // 16 voices, room for 64 plays and stops, and a mono device with one
    /// // input.
    /// let capture = Capture { channels: 1, backlog: 2048, inputs: 1 };
    /// let (control, mut renderer, mut feeder) = engine::create_with_capture(64, 16, capture);
    /// let mut input = control.open_input(InputChannels::Mono(0))?;
    ///
    /// // A capture callback would own the feeder.
    /// let capture = thread::spawn(move || {
    ///     let period = [0.0; 512];
    ///     let written = feeder.feed(&period);
    ///     println!("{written} frames in");
    /// });
    /// capture.join().expect("capture thread");
    ///
    /// // An output callback would own the renderer.
    /// renderer.render(&mut [0.0; 512 * 2]);
    ///
    /// input.stop();
    /// renderer.render(&mut [0.0; 512 * 2]);
    /// println!("ended: {:?}", input.try_wait()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn open_input(&self, channels: InputChannels) -> Result<Input, ControlError> {
        if self.capture_channels == 0 {
            return Err(ControlError::NoCapture);
        }
        let highest = match channels {
            InputChannels::Mono(channel) => channel,
            InputChannels::Stereo(left, right) => left.max(right),
        };
        if highest >= self.capture_channels {
            return Err(ControlError::NoSuchChannel);
        }
        let permit = self.requests.inputs.try_reserve()?;
        Ok(Input::open(permit, channels))
    }

    /// Frees the audio of voices and recordings whose handles are gone,
    /// returning it to `AudioMemory`. `record` does this first; call it when
    /// the app goes quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.requests.take_in();
    }
}
