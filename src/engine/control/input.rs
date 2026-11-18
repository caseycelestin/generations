//! An input: capture channels the renderer routes to stereo.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use thiserror::Error;

use crate::engine::protocol::inputs::{InputChannels, InputData, InputState};
use crate::transfer::{self, ControlEnd};

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

    pub fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}

/// Stops an input still open, since nothing could stop it once the handle
/// is gone.
impl Drop for Input {
    fn drop(&mut self) {
        self.stop();
    }
}
