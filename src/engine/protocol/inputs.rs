//! What crosses for inputs.

use crate::request::Request;

#[derive(Debug)]
pub(crate) struct InputRequest {
    pub(crate) channels: InputChannels,
}

impl Request for InputRequest {
    type Reply = InputEnd;
    type Status = InputStatus;
}

/// The capture device's channels an input reads, counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChannels {
    Mono(usize),
    /// Left, then right.
    Stereo(usize, usize),
}

/// The renderer's answer, while it holds the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStatus {
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEnd {
    /// Its `stop` arrived, or its handle was dropped.
    Stopped,
    /// Its feeder was dropped.
    FeederGone,
}
