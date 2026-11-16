//! The engine's face on the request channels: one per actor, and their
//! errors and answers in the engine's terms.

use thiserror::Error;

use crate::engine::protocol::recording::RecordingRequest;
use crate::request;

/// No reply yet, already taken, or never coming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryWaitError {
    /// It may still be running, or its end is on the way.
    #[error("it has not ended yet")]
    NotEnded,
    #[error("its result was already taken")]
    Taken,
    /// What it held went with the renderer.
    #[error("the renderer is gone, and what it held with it")]
    RendererGone,
}

impl From<request::TryWaitError> for TryWaitError {
    fn from(error: request::TryWaitError) -> Self {
        match error {
            request::TryWaitError::Empty => Self::NotEnded,
            request::TryWaitError::Closed => Self::Taken,
            request::TryWaitError::Disconnected => Self::RendererGone,
        }
    }
}

/// Nothing was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StatusError {
    /// An earlier question has not been answered yet.
    #[error("a status question is still out")]
    Busy,
    /// Its result is in `try_wait`.
    #[error("it has ended")]
    Ended,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<request::StatusError> for StatusError {
    fn from(error: request::StatusError) -> Self {
        match error {
            request::StatusError::Busy => Self::Busy,
            request::StatusError::Closed => Self::Ended,
            request::StatusError::Disconnected => Self::RendererGone,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TryRecvStatusError {
    #[error("the renderer has not answered yet")]
    NotAnswered,
    #[error("the answer was already taken")]
    Taken,
    /// Its result is in `try_wait`.
    #[error("it ended before the renderer answered")]
    Ended,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<request::TryRecvStatusError> for TryRecvStatusError {
    fn from(error: request::TryRecvStatusError) -> Self {
        match error {
            request::TryRecvStatusError::Empty => Self::NotAnswered,
            request::TryRecvStatusError::Taken => Self::Taken,
            request::TryRecvStatusError::Closed => Self::Ended,
            request::TryRecvStatusError::Disconnected => Self::RendererGone,
        }
    }
}

/// One request channel per actor in the renderer.
#[derive(Debug, Clone)]
pub(crate) struct Requests {
    pub(crate) recording: request::ControlEnd<RecordingRequest>,
}
