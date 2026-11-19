//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::engine::input::{Input, InputState};
use crate::engine::transfer;
use crate::layout::ChannelLayout;

#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    pub(crate) stop: AtomicBool,
}

/// Its audio comes back holding what was heard until the recording ended.
#[derive(Debug)]
pub(crate) struct RecordingData {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) tap: TapData,
    pub(crate) state: Arc<RecordingState>,
}

/// Where a recording takes its frames from, chosen per take.
#[derive(Debug, Clone, Copy)]
pub enum Tap<'a> {
    /// As heard, after clipping.
    MainOut,
    /// Whether it is monitored or not.
    Input(&'a Input),
}

/// A `Tap` owned, to cross to the renderer.
#[derive(Debug)]
pub(crate) enum TapData {
    MainOut,
    Input(Arc<InputState>),
}

/// Nothing was recorded; the audio comes back in it.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// A voice holds its audio until it ends; stop it, or wait for it to
    /// end.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// A stopped recording gives its place back at the renderer's next
    /// period.
    #[error("another recording is running")]
    Full(Audio),
    #[error("the renderer is gone")]
    RendererGone(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio)
            | Self::AudioInUse(audio)
            | Self::Full(audio)
            | Self::RendererGone(audio) => audio,
        }
    }
}

impl From<transfer::StartError<RecordingData>> for RecordError {
    fn from(error: transfer::StartError<RecordingData>) -> Self {
        match error {
            transfer::StartError::Full(data) => Self::Full(Audio::from_shared(data.audio)),
            transfer::StartError::Disconnected(data) => {
                Self::RendererGone(Audio::from_shared(data.audio))
            }
        }
    }
}

/// The audio was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryTakeError {
    /// Still recording, or its audio is on the way back.
    #[error("still recording")]
    Recording,
    #[error("the audio was already taken")]
    Taken,
    /// Not expected: a dropped renderer hands its recording back.
    #[error("the renderer is gone, and the audio with it")]
    RendererGone,
}

impl From<transfer::TryWaitError> for TryTakeError {
    fn from(error: transfer::TryWaitError) -> Self {
        match error {
            transfer::TryWaitError::Empty => Self::Recording,
            transfer::TryWaitError::Taken => Self::Taken,
            transfer::TryWaitError::Disconnected => Self::RendererGone,
        }
    }
}

/// Holds the recorder until the recording ends.
#[derive(Debug)]
pub struct Recording {
    transfer: transfer::Transfer<RecordingData>,
    state: Arc<RecordingState>,
}

impl Recording {
    pub(crate) fn record(
        transfers: &mut transfer::ControlEnd<RecordingData>,
        audio: Audio,
        tap: Tap<'_>,
    ) -> Result<Self, RecordError> {
        let audio = Self::validate(audio)?;
        let audio = Self::prepare(audio);
        let tap = match tap {
            Tap::MainOut => TapData::MainOut,
            Tap::Input(input) => TapData::Input(Arc::clone(input.state())),
        };
        let state = Arc::new(RecordingState::default());
        let data = RecordingData {
            audio,
            tap,
            state: Arc::clone(&state),
        };
        let transfer = transfers.try_start(data)?;
        Ok(Self { transfer, state })
    }

    pub fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }

    /// Tries to take the audio out, once it is back.
    pub fn try_take(&mut self) -> Result<Audio, TryTakeError> {
        let data = self.transfer.try_wait()?;
        Ok(Audio::from_shared(data.audio))
    }

    /// Ended voices must have been collected first, or they still count as
    /// holding `audio`.
    fn validate(audio: Audio) -> Result<Audio, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        // The caller gave up theirs, so nothing can clone it: the count can
        // only fall before the send.
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        Ok(audio)
    }

    /// `audio` must have passed `validate`.
    fn prepare(audio: Audio) -> Arc<AudioBuffer> {
        let mut audio = audio.into_shared();
        let buffer = Arc::get_mut(&mut audio).expect("checked: the only reference");
        buffer.clear();
        buffer.prefault();
        audio
    }
}

/// Stops a recording still running, since nothing could take its audio
/// once the handle is gone.
impl Drop for Recording {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Recordings at once: each holds a place from `record` until it comes
/// back.
const RECORDERS: usize = 1;

pub(crate) fn channel() -> (transfer::ControlEnd<RecordingData>, realtime::Recorder) {
    let (control, realtime) = transfer::channel(RECORDERS);
    (control, realtime::Recorder::new(realtime))
}

/// The renderer's one recorder: it writes at most one recording at a time.
pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::{RecordingData, TapData};
    use crate::engine::input::realtime::Input;
    use crate::engine::renderer::Frame;
    use crate::engine::transfer::{self, Completion};

    /// A point in the render loop a take may record from.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum TapPoint<'a> {
        MainOut,
        Input(&'a Input),
    }

    #[derive(Debug)]
    pub(crate) struct Recorder {
        recording: Option<Completion<RecordingData>>,
        transfers: transfer::RealtimeEnd<RecordingData>,
    }

    impl Recorder {
        pub(crate) fn new(transfers: transfer::RealtimeEnd<RecordingData>) -> Self {
            Self {
                recording: None,
                transfers,
            }
        }

        /// A new recording starts, then a stopped one ends, including one
        /// stopped before it arrived.
        pub(crate) fn receive(&mut self) {
            if let Some(recording) = self.transfers.try_recv() {
                debug_assert!(self.recording.is_none(), "one transfer per recorder");
                self.recording = Some(recording);
            }
            if let Some(stopped) = self.recording.take_if(|recording| Self::stopped(recording)) {
                self.end(stopped);
            }
        }

        /// Never true with no take.
        pub(crate) fn records_from(&self, point: TapPoint<'_>) -> bool {
            let Some(recording) = &self.recording else {
                return false;
            };
            match (point, &recording.tap) {
                (TapPoint::MainOut, TapData::MainOut) => true,
                (TapPoint::Input(input), TapData::Input(state)) => {
                    Arc::ptr_eq(input.state(), state)
                }
                (TapPoint::MainOut, TapData::Input(_)) | (TapPoint::Input(_), TapData::MainOut) => {
                    false
                }
            }
        }

        /// Appends `block`; a recording that fills ends here.
        pub(crate) fn write(&mut self, block: &[Frame]) {
            let Some(recording) = &mut self.recording else {
                return;
            };
            let audio = Arc::get_mut(&mut recording.audio);
            debug_assert!(audio.is_some(), "record refuses audio a voice holds");
            if let Some(audio) = audio {
                audio.extend(block.as_flattened().iter().copied());
            }
            if let Some(full) = self
                .recording
                .take_if(|recording| recording.audio.is_full())
            {
                self.end(full);
            }
        }

        /// Its handle stopped it or is gone, or its input's did: a take
        /// ends with its input.
        fn stopped(recording: &Completion<RecordingData>) -> bool {
            let input_stopped = match &recording.tap {
                TapData::MainOut => false,
                TapData::Input(input) => input.stop.load(Ordering::Relaxed),
            };
            recording.state.stop.load(Ordering::Relaxed) || input_stopped
        }

        /// The one way a recording leaves.
        fn end(&mut self, recording: Completion<RecordingData>) {
            self.transfers.complete(recording);
        }
    }

    /// A recording's audio is its caller's, so it goes home even when the
    /// renderer goes; one still on its way is received first.
    impl Drop for Recorder {
        fn drop(&mut self) {
            self.receive();
            if let Some(recording) = self.recording.take() {
                self.end(recording);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;

        use super::*;
        use crate::audio::AudioMemory;
        use crate::engine::recording::{RecordingState, TapData};
        use crate::engine::transfer::{ControlEnd, Transfer, TryWaitError};
        use crate::layout::ChannelLayout;

        fn recorder() -> (ControlEnd<RecordingData>, Recorder) {
            let (control, realtime) = transfer::channel(1);
            (control, Recorder::new(realtime))
        }

        /// Starts a recording into empty stereo audio with room for `frames`,
        /// as `Control::record` does, keeping the state its handle stops it
        /// through.
        fn record(
            control: &mut ControlEnd<RecordingData>,
            frames: usize,
        ) -> (Transfer<RecordingData>, Arc<RecordingState>) {
            let audio = AudioMemory::new()
                .alloc_buffer(ChannelLayout::Stereo, frames)
                .unwrap();
            let state = Arc::new(RecordingState::default());
            let data = RecordingData {
                audio: Arc::new(audio),
                tap: TapData::MainOut,
                state: Arc::clone(&state),
            };
            let recording = control.try_start(data).unwrap();
            (recording, state)
        }

        #[test]
        fn a_recording_stopped_before_it_arrives_ends_at_once_empty() {
            let (mut control, mut recorder) = recorder();
            let (mut recording, state) = record(&mut control, 64);
            state.stop.store(true, Ordering::Relaxed);

            recorder.receive();

            let data = recording.try_wait().unwrap();
            assert_eq!(data.audio.frames(), 0);
        }

        #[test]
        fn a_take_ends_in_the_block_that_fills_it() {
            let (mut control, mut recorder) = recorder();
            let (mut recording, _) = record(&mut control, 4);
            recorder.receive();

            recorder.write(&[[0.5; 2]; 2]);
            assert_eq!(recording.try_wait().err(), Some(TryWaitError::Empty));
            recorder.write(&[[0.5; 2]; 2]);

            let data = recording.try_wait().unwrap();
            assert_eq!(data.audio.frames(), 4);
        }
    }
}
