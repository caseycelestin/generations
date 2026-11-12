//! A voice as the renderer holds it while it plays.

use crate::engine::protocol::voice::VoiceSpec;

#[derive(Debug)]
pub(crate) struct Voice {
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    /// Only moves the spec in, so nothing allocates.
    pub(crate) fn new(spec: VoiceSpec) -> Self {
        Self { spec, position: 0 }
    }
}
