//! The control side: runs on UI or event threads. May lock and allocate.

mod input;
mod recording;
mod requests;
mod transfers;
mod voice;

pub use input::{Input, PendingInputStatus};
pub use recording::{PendingRecordingStatus, RecordError, Recording};
pub(crate) use requests::Requests;
pub use requests::{ControlError, StatusError, TryRecvStatusError, TryWaitError};
pub use voice::{PlayError, Voice};

use crate::audio::Audio;
use crate::engine::protocol::inputs::InputChannels;

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
    /// use generations::engine::{self, InputChannels};
    ///
    /// // 16 voices, room for 64 plays, and a mono device with one input.
    /// let (control, mut renderer) = engine::create(64, 16, 1, 1);
    /// let mut input = control.open_input(InputChannels::Mono(0))?;
    ///
    /// // A duplex callback would own the renderer: a period of capture in,
    /// // the same frames out.
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    ///
    /// input.stop();
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    /// println!("ended: {:?}", input.try_wait()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn open_input(&self, channels: InputChannels) -> Result<Input, ControlError> {
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
