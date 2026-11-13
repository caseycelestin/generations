//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::num::NonZeroUsize;

use crate::engine::protocol::voice::VoiceSpec;

/// A voice while it plays.
#[derive(Debug)]
struct Voice {
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    /// Only moves the spec in, so nothing allocates.
    fn new(spec: VoiceSpec) -> Self {
        Self { spec, position: 0 }
    }
}

#[derive(Debug)]
pub(crate) struct Voices {
    /// A fixed number of places: `None` is free. A boxed slice cannot
    /// grow, so nothing here reallocates on the audio thread.
    places: Box<[Option<Voice>]>,
}

impl Voices {
    /// `count` free places, allocated here, on the control side. At least
    /// one, so an `Add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
        }
    }
}
