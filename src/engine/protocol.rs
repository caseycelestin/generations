//! What crosses between the two sides. Owned by neither.
//!
//! Commands are operations on what the renderer holds (add, remove), not
//! user intents; handles translate one into the other. One command type
//! per feature. Only plain data crosses: audio, ids, values.

use std::sync::Arc;

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum Command {
    Sample(SampleCommand),
}

#[derive(Debug)]
pub(crate) enum SampleCommand {
    /// Play `audio` from its first frame.
    Add {
        id: SoundId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop the sound, if it is still playing.
    Remove(SoundId),
}

/// Handed back so the last reference is dropped on the control side:
/// freeing has no time bound and must not happen on the audio thread.
#[derive(Debug)]
pub(crate) enum Return {
    /// A sample sound's audio: finished, removed or cut.
    Sample(Arc<AudioBuffer>),
}

/// One instance of something heard. Minted in order by the control side,
/// so the lowest id is the oldest. Never reused, so a stale remove does
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SoundId(pub(crate) u64);
