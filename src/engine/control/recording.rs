//! A recording: the main out written into audio the renderer holds.

use std::sync::Arc;

use thiserror::Error;

use super::ControlError;
use crate::audio::{Audio, AudioBuffer};
use crate::collector::Collector;
use crate::duplex::{ControlEnd, Handle, ReserveError};
use crate::engine::protocol::recording::{RecordingCommand, RecordingEvent, RecordingState};
use crate::engine::protocol::{Command, Event};
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
    /// Voices and a recording not yet ended hold every event slot; nothing
    /// was sent.
    #[error("every event slot is held")]
    EventsFull(Audio),
}

impl RecordError {
    pub fn into_audio(self) -> Audio {
        match self {
            Self::NotStereo(audio)
            | Self::AudioInUse(audio)
            | Self::CommandsFull(audio)
            | Self::EventsFull(audio) => audio,
        }
    }

    fn refused(error: ReserveError, audio: Audio) -> Self {
        match error {
            ReserveError::CommandsFull => Self::CommandsFull(audio),
            ReserveError::EventsFull => Self::EventsFull(audio),
        }
    }
}

#[derive(Debug)]
pub struct Recording {
    handle: Handle<Command, Event>,
    state: RecordingState,
    /// The audio once it comes back, until taken.
    audio: Option<Arc<AudioBuffer>>,
}

impl Recording {
    pub(crate) fn record(
        channel: &ControlEnd<Command, Event>,
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
        // Two events: its start, and its end.
        let permit = match channel.try_reserve_many(2) {
            Ok(permit) => permit,
            Err(error) => return Err(RecordError::refused(error, Audio::from_shared(audio))),
        };
        let handle = permit.open(RecordingCommand::Start(audio));
        Ok(Self {
            handle,
            state: RecordingState::Pending,
            audio: None,
        })
    }

    pub fn stop(&self) -> Result<(), ControlError> {
        self.handle
            .send(RecordingCommand::Stop)
            .map_err(ControlError::from)
    }

    pub fn state(&mut self) -> RecordingState {
        self.receive();
        self.state
    }

    /// The audio, once the renderer has let go of it: after the recording
    /// ends, or at once if it was refused. `None` until then, and after it
    /// has been taken.
    pub fn take(&mut self) -> Option<Audio> {
        self.receive();
        self.audio.take().map(Audio::from_shared)
    }

    fn receive(&mut self) {
        while let Some(event) = self.handle.try_recv() {
            let Event::Recording(event) = event else {
                unreachable!("a handle hears only its own feature's events");
            };
            match event {
                RecordingEvent::Started => self.state = RecordingState::Recording,
                RecordingEvent::Refused(audio) => {
                    self.state = RecordingState::Refused;
                    self.audio = Some(audio);
                }
                RecordingEvent::Ended(why, audio) => {
                    self.state = RecordingState::Ended(why);
                    self.audio = Some(audio);
                }
            }
        }
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
