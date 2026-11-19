//! The renderer's one recorder: it writes at most one recording at a time.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::Frame;
use crate::engine::protocol::recording::RecordingData;
use crate::transfer::{self, Completion};

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

    /// Appends `block`, as heard; a recording that fills ends here.
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

    /// Its handle stopped it, or is gone.
    fn stopped(recording: &Completion<RecordingData>) -> bool {
        recording.state.stop.load(Ordering::Relaxed)
    }

    /// The one way a recording leaves: its audio goes home.
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
    use crate::engine::protocol::recording::RecordingState;
    use crate::layout::ChannelLayout;
    use crate::transfer::{ControlEnd, Transfer, TryWaitError};

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
