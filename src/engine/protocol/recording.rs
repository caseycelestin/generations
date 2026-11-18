//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;

use crate::audio::AudioBuffer;

/// Comes back whole once the recording ends, its audio holding what was
/// heard until then.
#[derive(Debug)]
pub(crate) struct RecordingData {
    pub(crate) audio: Arc<AudioBuffer>,
}
