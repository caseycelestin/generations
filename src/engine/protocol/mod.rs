//! What crosses between the two sides, owned by neither.
//!
//! Each request is a task the renderer carries out, answered once it is
//! done; handles stand for them on the control side. Only plain data
//! crosses: audio, ids and values. Each feature has its own file and its
//! own request channel.

use std::sync::Arc;

use crate::audio::AudioBuffer;

pub(crate) mod recording;
pub(crate) mod voice;

/// Names audio to the renderer without sending it, so nothing comes back
/// to be freed. Sound only while the audio cannot be freed before the
/// request is applied: a later buffer at the same address then starts
/// behind it on the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferAddress(usize);

impl BufferAddress {
    pub(crate) fn of(audio: &Arc<AudioBuffer>) -> Self {
        Self(Arc::as_ptr(audio) as usize)
    }
}
