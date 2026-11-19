use crate::audio::Audio;
use crate::engine::input::{Input, InputChannels, InputData, InputError};
use crate::engine::recording::{RecordError, Recording, RecordingData, Tap};
use crate::engine::transfer;
use crate::engine::voice::{Voice, VoiceData, VoiceError};

/// One per renderer; an app shares it its own way. Everything done here or
/// on a handle takes effect at the renderer's next period.
#[derive(Debug)]
pub struct Control {
    voices: transfer::ControlEnd<VoiceData>,
    inputs: transfer::ControlEnd<InputData>,
    recordings: transfer::ControlEnd<RecordingData>,
    /// The capture device's, or zero without one.
    capture_channels: usize,
}

impl Control {
    pub(crate) fn new(
        voices: transfer::ControlEnd<VoiceData>,
        inputs: transfer::ControlEnd<InputData>,
        recordings: transfer::ControlEnd<RecordingData>,
        capture_channels: usize,
    ) -> Self {
        Self {
            voices,
            inputs,
            recordings,
            capture_channels,
        }
    }

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
    pub fn play(&mut self, audio: &Audio) -> Result<Voice, VoiceError> {
        Voice::play(&mut self.voices, audio)
    }

    /// Records what `tap` gives into `audio`, from empty. By value: the
    /// renderer writes it until it comes back. Refused while another
    /// recording runs, or while a voice holds `audio`.
    ///
    /// ```no_run
    /// use generations::engine::{self, Tap};
    /// use generations::{AudioMemory, ChannelLayout};
    ///
    /// let (mut control, mut renderer) = engine::create(64, 16, 0, 0);
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
    /// // 16 voices, room for 64 plays, and a mono device with one input.
    /// let (mut control, mut renderer) = engine::create(64, 16, 1, 1);
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
        Input::open(&mut self.inputs, channels, self.capture_channels)
    }

    /// Frees the audio of voices that have ended and of recordings whose
    /// handles are gone, returning it to `AudioMemory`. `record` does this
    /// first; call it when the app goes quiet, or before a load that needs
    /// the memory.
    pub fn collect(&mut self) {
        self.voices.take_in();
        self.inputs.take_in();
        self.recordings.take_in();
    }
}
