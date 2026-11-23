use std::sync::Arc;

use rtrb::Producer;

use crate::audio::Audio;
use crate::engine::audio::{self, AudioBuffer, AudioState, LoadError};
use crate::engine::input::{Input, InputChannels, InputData, InputError, InputState};
use crate::engine::pool::Pool;
use crate::engine::recording::{RecordError, Recording, RecordingData, Tap};
use crate::engine::recording_stream::Recorders;
use crate::engine::transfer;
use crate::layout::ChannelLayout;

/// One per renderer; an app shares it its own way. Everything done here or
/// on a handle takes effect at the renderer's next period.
#[derive(Debug)]
pub struct Control {
    inputs: Producer<InputData>,
    input_places: Pool<InputState>,
    audio: Producer<Arc<AudioState>>,
    audio_places: Pool<AudioState>,
    recordings: transfer::ControlEnd<RecordingData>,
    recorders: Recorders,
    /// The capture device's, or zero without one.
    capture_channels: usize,
}

impl Control {
    pub(crate) fn new(
        inputs: Producer<InputData>,
        input_places: Pool<InputState>,
        audio: Producer<Arc<AudioState>>,
        audio_places: Pool<AudioState>,
        recordings: transfer::ControlEnd<RecordingData>,
        recorders: Recorders,
        capture_channels: usize,
    ) -> Self {
        Self {
            inputs,
            input_places,
            audio,
            audio_places,
            recordings,
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
    /// let (mut control, mut renderer) = engine::create(0, 0, 1, 8);
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

    /// Records what `tap` gives into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs.
    ///
    /// ```no_run
    /// use generations::engine::{self, Tap};
    /// use generations::{AudioMemory, ChannelLayout};
    ///
    /// let (mut control, mut renderer) = engine::create(0, 0, 0, 8);
    /// // Room for a second of the main out.
    /// let take = AudioMemory::new().alloc(ChannelLayout::Stereo, 48_000)?;
    /// let mut recording = control.record(take, Tap::MainOut)?;
    ///
    /// // Written from the renderer's next period until stopped or full;
    /// // back, holding what was heard, at the period after.
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// recording.stop();
    /// renderer.render(&[], &mut [0.0; 512 * 2]);
    /// let recorded = recording.try_take()?;
    /// assert_eq!(recorded.frames(), 512);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn record(&mut self, audio: Audio, tap: Tap<'_>) -> Result<Recording, RecordError> {
        self.collect();
        Recording::record(&mut self.recordings, audio, tap)
    }

    /// Inputs may read the same channels; each gets its own copy.
    ///
    /// ```no_run
    /// use generations::engine::{self, InputChannels};
    ///
    /// // A mono device, with one input.
    /// let (mut control, mut renderer) = engine::create(1, 1, 0, 8);
    /// let mut input = control.open_input(InputChannels::Mono(0))?;
    /// // Heard through the main out until stopped.
    /// input.set_monitoring(true);
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
        let highest = match channels {
            InputChannels::Mono(channel) => channel,
            InputChannels::Stereo(left, right) => left.max(right),
        };
        if highest >= self.capture_channels {
            return Err(InputError::NoSuchChannel);
        }
        if self.inputs.is_abandoned() {
            return Err(InputError::RendererGone);
        }
        let state = self
            .input_places
            .take(InputState::default(), |state, fresh| *state = fresh)
            .map_err(|_| InputError::Full)?;
        let data = InputData {
            channels,
            state: Arc::clone(&state),
        };
        // Each message holds a place, so the ring never has more than places.
        self.inputs
            .push(data)
            .expect("the ring holds one message per place");
        Ok(Input::open(state))
    }

    /// Frees the audio of recordings whose handles are gone, returning it to
    /// `AudioMemory`. `record` does this first; call it when the app goes
    /// quiet, or before a load that needs the memory.
    pub fn collect(&mut self) {
        self.recordings.take_in();
    }

    pub fn recorders(&mut self) -> &mut Recorders {
        &mut self.recorders
    }
}
