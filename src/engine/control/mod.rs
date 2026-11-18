//! The control side: runs on UI or event threads. May lock and allocate.

mod input;
mod recording;
mod requests;
mod transfers;
mod voice;

pub use input::{Input, InputError};
pub use recording::{PendingRecordingStatus, RecordError, Recording};
pub(crate) use requests::Requests;
pub use requests::{StatusError, TryRecvStatusError, TryWaitError};
pub(crate) use transfers::Transfers;
pub use voice::{Voice, VoiceError};

use crate::audio::Audio;
use crate::engine::protocol::inputs::InputChannels;

/// One per renderer; an app shares it its own way. Every request, from
/// here or from a handle, takes effect at the renderer's next period.
#[derive(Debug)]
pub struct Control {
    requests: Requests,
    transfers: Transfers,
    /// The capture device's, or zero without one.
    capture_channels: usize,
}

impl Control {
    pub(crate) fn new(requests: Requests, transfers: Transfers, capture_channels: usize) -> Self {
        Self {
            requests,
            transfers,
            capture_channels,
        }
    }

    pub fn play(&mut self, audio: &Audio) -> Result<Voice, VoiceError> {
        Voice::play(&mut self.transfers.voices, audio)
    }

    /// Records the main out into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs, or while a voice holds `audio`.
    pub fn record(&mut self, audio: Audio) -> Result<Recording, RecordError> {
        self.collect();
        Recording::record(&self.requests.recording, audio)
    }

    /// Inputs may read the same channels; each gets its own copy.
    ///
    /// ```no_run
    /// use generations::engine::{self, InputChannels};
    ///
    /// // 16 voices, room for 64 plays, and a mono device with one input.
    /// let (mut control, mut renderer) = engine::create(64, 16, 1, 1);
    /// let mut input = control.open_input(InputChannels::Mono(0))?;
    ///
    /// // A duplex callback would own the renderer: a period of capture in,
    /// // the same frames out.
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    ///
    /// // Silent, and its place free, from the renderer's next period.
    /// input.stop();
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn open_input(&mut self, channels: InputChannels) -> Result<Input, InputError> {
        Input::open(&mut self.transfers.inputs, channels, self.capture_channels)
    }

    /// Frees the audio of voices and recordings whose handles are gone,
    /// returning it to `AudioMemory`. `record` does this first; call it when
    /// the app goes quiet, or before a load that needs the memory.
    pub fn collect(&mut self) {
        self.requests.take_in();
        self.transfers.take_in();
    }
}
