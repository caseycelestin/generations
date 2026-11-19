//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::audio::AudioBuffer;

/// Shared by both sides while the recording runs.
#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    pub(crate) stop: AtomicBool,
}

/// Comes back whole once the recording ends, its audio holding what was
/// heard until then.
#[derive(Debug)]
pub(crate) struct RecordingData {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) state: Arc<RecordingState>,
}
