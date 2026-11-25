use std::sync::Arc;

use rtrb::Producer;

use crate::engine::audio::{self, AudioBuffer, AudioState, LoadError};
use crate::engine::input::InputState;
use crate::engine::input_v2;
use crate::engine::pool::Pool;
use crate::engine::recording::Recorders;
use crate::layout::ChannelLayout;

/// One per renderer; an app shares it its own way. Everything done here or
/// on a handle takes effect at the renderer's next period.
#[derive(Debug)]
pub struct Control {
    inputs: input_v2::Inputs,
    input_places: Pool<InputState>,
    audio: Producer<Arc<AudioState>>,
    audio_places: Pool<AudioState>,
    recorders: Recorders,
    /// The capture device's, or zero without one.
    capture_channels: usize,
}

impl Control {
    pub(crate) fn new(
        inputs: input_v2::Inputs,
        input_places: Pool<InputState>,
        audio: Producer<Arc<AudioState>>,
        audio_places: Pool<AudioState>,
        recorders: Recorders,
        capture_channels: usize,
    ) -> Self {
        Self {
            inputs,
            input_places,
            audio,
            audio_places,
            recorders,
            capture_channels,
        }
    }

    /// The engine holds `samples` until the audio is unloaded; a refused
    /// load hands them back.
    ///
    /// ```no_run
    /// use generations::{ChannelLayout, engine};
    ///
    /// // Room to load one audio, with eight voices of its own.
    /// let (mut control, mut renderer) = engine::create(0, 0, 1, 8, 0, 0);
    /// let mut pluck = control.load(vec![0.5; 48_000], ChannelLayout::Mono)?;
    /// let voice = pluck.play();
    ///
    /// // Heard from the renderer's next block until its handle drops.
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// drop(voice);
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn load(
        &mut self,
        samples: Vec<f32>,
        layout: ChannelLayout,
    ) -> Result<audio::Audio, LoadError> {
        if !samples.len().is_multiple_of(layout.count()) {
            return Err(LoadError::PartFrame(samples));
        }
        if self.audio.is_abandoned() {
            return Err(LoadError::RendererGone(samples));
        }
        let buffer = AudioBuffer { samples, layout };
        let state = self
            .audio_places
            .take(buffer, AudioState::refill)
            .map_err(|buffer| LoadError::Full(buffer.samples))?;
        // Each message holds a place, so the ring never has more than places.
        self.audio
            .push(Arc::clone(&state))
            .expect("the ring holds one message per place");
        Ok(audio::Audio::load(state))
    }

    pub fn inputs(&mut self) -> &mut input_v2::Inputs {
        &mut self.inputs
    }

    /// Arming one hands out a [`Recorder`](crate::engine::Recorder) that
    /// starts and stops it and a [`Drain`](crate::engine::Drain) that
    /// collects what it recorded, usually on a thread of its own.
    ///
    /// ```no_run
    /// use generations::engine;
    ///
    /// // One recorder, holding up to half a second its drain has yet to take.
    /// let (mut control, mut renderer) = engine::create(0, 0, 0, 8, 1, 24_000);
    /// let (recorder, mut drain) = control.recorders().arm()?;
    ///
    /// // Recorded from the renderer's next period until a period after the stop.
    /// recorder.record();
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// recorder.stop();
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    ///
    /// let mut recording = Vec::new();
    /// drain.drain_into(&mut recording);
    /// assert_eq!(recording.len(), 512 * 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn recorders(&mut self) -> &mut Recorders {
        &mut self.recorders
    }
}
