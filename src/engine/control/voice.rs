//! A voice: one playing of some audio.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{ControlError, TryWaitError};
use crate::audio::Audio;
use crate::engine::protocol::voice::{VoiceData, VoiceState};
use crate::transfer::{ControlEnd, Transfer};

/// Its voice holds a slot until it ends, whether the handle is kept, waited
/// on or dropped.
#[derive(Debug)]
pub struct Voice {
    handle: Transfer<VoiceData>,
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(
        transfers: &mut ControlEnd<VoiceData>,
        audio: &Audio,
    ) -> Result<Self, ControlError> {
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        let handle = transfers.try_start(data)?;
        Ok(Self { handle, state })
    }

    /// Dropping the handle instead lets the voice play out.
    ///
    /// ```no_run
    /// use generations::{AudioMemory, ChannelLayout, engine};
    ///
    /// let (mut control, mut renderer) = engine::create(64, 16, 0, 0);
    /// let audio = AudioMemory::new().alloc(ChannelLayout::Mono, 48_000)?;
    /// let mut voice = control.play(&audio)?;
    ///
    /// // Heard from the renderer's next period until stopped; silent from
    /// // the one after.
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// voice.stop();
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// println!("ended: {:?}", voice.try_wait());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(&self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }

    /// `Ok` once it has ended.
    pub fn try_wait(&mut self) -> Result<(), TryWaitError> {
        self.handle.try_wait()?;
        Ok(())
    }
}
