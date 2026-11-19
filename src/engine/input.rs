//! An input: capture channels the renderer routes to stereo.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::transfer::{self, ControlEnd};

/// Shared by both sides while the input is open.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    pub(crate) stop: AtomicBool,
}

#[derive(Debug)]
pub(crate) struct InputData {
    pub(crate) channels: InputChannels,
    pub(crate) state: Arc<InputState>,
}

/// The capture device's channels an input reads, counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChannels {
    Mono(usize),
    /// Left, then right.
    Stereo(usize, usize),
}

/// Nothing was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InputError {
    /// Past the capture device's channels.
    #[error("no such channel")]
    NoSuchChannel,
    /// A stopped input gives its place back at the renderer's next period.
    #[error("every input is open")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<transfer::StartError<InputData>> for InputError {
    fn from(error: transfer::StartError<InputData>) -> Self {
        match error {
            transfer::StartError::Full(_) => Self::Full,
            transfer::StartError::Disconnected(_) => Self::RendererGone,
        }
    }
}

#[derive(Debug)]
pub struct Input {
    state: Arc<InputState>,
}

impl Input {
    pub(crate) fn open(
        transfers: &mut ControlEnd<InputData>,
        channels: InputChannels,
        capture_channels: usize,
    ) -> Result<Self, InputError> {
        let highest = match channels {
            InputChannels::Mono(channel) => channel,
            InputChannels::Stereo(left, right) => left.max(right),
        };
        if highest >= capture_channels {
            return Err(InputError::NoSuchChannel);
        }
        let state = Arc::new(InputState::default());
        let data = InputData {
            channels,
            state: Arc::clone(&state),
        };
        transfers.try_start(data)?;
        Ok(Self { state })
    }

    /// The same as dropping it.
    pub fn stop(self) {}
}

/// Unlike a voice, an input never ends by itself, so it ends with its
/// handle.
impl Drop for Input {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}
