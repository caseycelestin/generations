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
use crate::engine::protocol::voice::VoiceRequest;
use crate::engine::realtime::capture::Feeder;
use crate::layout::ChannelLayout;
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
}

impl Control {
    pub(crate) fn new(requests: Requests) -> Self {
        Self { requests }
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

    /// `backlog` is how many frames can wait for the renderer's next period
    /// before any are dropped: at least the longest output period, with
    /// room to spare when the capture period is longer.
    ///
    /// ```no_run
    /// use std::thread;
    ///
    /// use generations::ChannelLayout;
    /// use generations::engine;
    ///
    /// // 16 voices, room for 64 plays and stops, and one input.
    /// let (control, mut renderer) = engine::create(64, 16, 1);
    /// let (mut input, mut feeder) = control.open_input(ChannelLayout::Mono, 2048)?;
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
    pub fn open_input(
        &self,
        layout: ChannelLayout,
        backlog: usize,
    ) -> Result<(Input, Feeder), ControlError> {
        let permit = self.requests.inputs.try_reserve()?;
        let (feeder, reader) = Feeder::new(layout, backlog);
        Ok((Input::open(permit, reader, layout), feeder))
    }

    /// Frees the audio of voices and recordings whose handles are gone,
    /// returning it to `AudioMemory`. `record` does this first; call it when
    /// the app goes quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.requests.take_in();
    }
}
