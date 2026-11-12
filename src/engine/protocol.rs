//! What crosses between the two sides. Owned by neither.
//!
//! Commands are operations on what the renderer holds (add, remove), not
//! user intents; handles translate one into the other. One command type
//! per feature. Only plain data crosses: audio, ids, values.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum Command {
    Voice(VoiceCommand),
}

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    /// Play `audio` from its first frame.
    Add {
        id: VoiceId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop the voice, if it is still playing.
    Remove(VoiceId),
}

/// Handed back so the last reference is dropped on the control side:
/// freeing has no time bound and must not happen on the audio thread.
#[derive(Debug)]
pub(crate) enum Return {
    /// A voice's audio: finished, removed or cut.
    Voice(Arc<AudioBuffer>),
}

/// Defines an id type minted by `next()` from its own process-wide
/// counter, as `std::thread::ThreadId` is: unique and in order across
/// every engine, never reused, not starting at 0 per engine. A macro
/// because a `static` in a generic function would be shared by every type.
macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) struct $name(u64);

        impl $name {
            pub(crate) fn next() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                Self(NEXT.fetch_add(1, Ordering::Relaxed))
            }
        }
    };
}

id! {
    /// One playing of a sample. The lowest id is the oldest; a stale
    /// remove names nothing and does nothing.
    VoiceId
}
