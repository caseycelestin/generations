//! A voice: one playing of some audio.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use thiserror::Error;

use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceData, VoiceState};
use crate::transfer::{self, ControlEnd};

/// Nothing was played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VoiceError {
    /// Stop one, or wait for one to end, then try again.
    #[error("every voice is held")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<transfer::StartError<VoiceData>> for VoiceError {
    fn from(error: transfer::StartError<VoiceData>) -> Self {
        match error {
            transfer::StartError::Full(_) => Self::Full,
            transfer::StartError::Disconnected(_) => Self::RendererGone,
        }
    }
}

/// Its voice holds a slot until it ends, whether the handle is kept or
/// dropped.
#[derive(Debug)]
pub struct Voice {
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(
        transfers: &mut ControlEnd<VoiceData>,
        audio: &Audio,
    ) -> Result<Self, VoiceError> {
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        transfers.try_start(data)?;
        Ok(Self { state })
    }

    /// Dropping the handle instead lets the voice play out.
    ///
    /// ```no_run
    /// use generations::{AudioMemory, ChannelLayout, engine};
    ///
    /// let (mut control, mut renderer) = engine::create(64, 16, 0, 0);
    /// let audio = AudioMemory::new().alloc(ChannelLayout::Mono, 48_000)?;
    /// let voice = control.play(&audio)?;
    ///
    /// // Heard from the renderer's next period until stopped; silent from
    /// // the one after.
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// voice.stop();
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(&self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}
