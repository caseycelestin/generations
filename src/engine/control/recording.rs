//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use super::ControlError;
use super::handle::{Handle, Inbox};
use super::rings::SharedRings;
use crate::audio::{Audio, AudioBuffer};
use crate::collector::Collector;
use crate::engine::protocol::Event;
use crate::engine::protocol::recording::{
    RecordingCommand, RecordingEvent, RecordingSpec, RecordingState,
};
use crate::layout::ChannelLayout;

/// A refused `record`, holding the audio it was given.
#[derive(Debug, Error)]
pub enum RecordError {
    /// The main out is stereo, so mono audio cannot hold it.
    #[error("recording needs stereo audio")]
    NotStereo(Audio),
    /// Stop its voices and try again.
    #[error("a voice still holds the audio")]
    AudioInUse(Audio),
    /// The renderer has not caught up; nothing was sent.
    #[error("the command ring is full")]
    CommandsFull(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio) | Self::AudioInUse(audio) | Self::CommandsFull(audio) => audio,
        }
    }
}

#[derive(Debug)]
pub struct Recording {
    handle: Handle<RecordingEvent>,
}

impl Recording {
    pub(crate) fn record(
        rings: SharedRings,
        collector: &Collector<Arc<AudioBuffer>>,
        audio: Audio,
    ) -> Result<Self, RecordError> {
        if audio.layout() != ChannelLayout::Stereo {
            return Err(RecordError::NotStereo(audio));
        }
        // Collected first, so ended voices have let go. The caller gave up
        // theirs, so nothing can clone it: the count can only fall before
        // the send.
        collector.collect();
        if Arc::strong_count(audio.shared()) > 1 {
            return Err(RecordError::AudioInUse(audio));
        }
        let mut audio = audio.into_shared();
        let buffer = Arc::get_mut(&mut audio).expect("checked: the only reference");
        buffer.clear();
        buffer.prefault();
        let spec = RecordingSpec { audio };
        match Handle::open(&rings, spec, RecordingCommand::Start) {
            Ok(handle) => Ok(Self { handle }),
            Err(spec) => Err(RecordError::CommandsFull(Audio::from_shared(spec.audio))),
        }
    }

    pub fn stop(&self) -> Result<(), ControlError> {
        self.handle.send(RecordingCommand::Stop)
    }

    pub fn state(&self) -> RecordingState {
        self.handle.inbox().state
    }

    /// The audio, once the renderer has let go of it: after the recording
    /// ends, or at once if it was refused. `None` until then, and after it
    /// has been taken.
    pub fn take(&mut self) -> Option<Audio> {
        let spec = self.handle.inbox().spec.take();
        spec.map(|spec| Audio::from_shared(spec.audio))
    }
}

/// Keeps the last event whole, so its spec waits here for `take`. The event
/// that hands the spec back is the last.
impl Inbox for RecordingEvent {
    fn deliver(&mut self, event: Event) -> bool {
        let Event::Recording(event) = event else {
            unreachable!("an id belongs to one feature");
        };
        let last = event.spec.is_some();
        *self = event;
        last
    }
}

/// Stops a recording still running, since nothing could take its audio
/// once the handle is gone. A full ring loses the stop; the take then runs
/// until its audio is full.
impl Drop for Recording {
    fn drop(&mut self) {
        if matches!(
            self.state(),
            RecordingState::Pending | RecordingState::Recording
        ) {
            let _ = self.stop();
        }
    }
}
