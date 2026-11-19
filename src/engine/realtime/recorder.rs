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
