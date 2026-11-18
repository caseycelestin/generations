//! What crosses for inputs.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
