//! What crosses between the two sides. Owned by neither.
//!
//! Commands are operations on what the renderer holds (add, remove), not
//! user intents; handles translate one into the other. One command type
//! per feature. Only plain data crosses: audio, ids, values, and reports
//! the renderer fills with one atomic store.
//!
//! Each feature's vocabulary lives in its own file; this one holds what
//! they share. The rings carry one type each way, so [`Command`] and
//! [`Return`] list a variant per feature.

use std::sync::Arc;

use crate::audio::AudioBuffer;

/// Defines an id type minted by `next()` from its own process-wide
/// counter, as `std::thread::ThreadId` is: unique and in order across
/// every engine, never reused, not starting at 0 per engine. A macro
/// because a `static` in a generic function would be shared by every type.
/// Defined before the feature modules so they can use it.
macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) struct $name(u64);

        impl $name {
            pub(crate) fn next() -> Self {
                static NEXT: ::std::sync::atomic::AtomicU64 =
                    ::std::sync::atomic::AtomicU64::new(0);
                Self(NEXT.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed))
            }
        }
    };
}

pub(crate) mod recording;
pub(crate) mod voice;

use recording::{RecordingCommand, RecordingReport};
use voice::{VoiceCommand, VoiceReport};

#[derive(Debug)]
pub(crate) enum Command {
    Voice(VoiceCommand),
    Recording(RecordingCommand),
}

impl From<VoiceCommand> for Command {
    fn from(command: VoiceCommand) -> Self {
        Self::Voice(command)
    }
}

impl From<RecordingCommand> for Command {
    fn from(command: RecordingCommand) -> Self {
        Self::Recording(command)
    }
}

/// Handed back so the last reference is dropped on the control side:
/// freeing has no time bound and must not happen on the audio thread.
#[derive(Debug)]
pub(crate) enum Return {
    /// A voice ended; its report is already filled in.
    Voice {
        audio: Arc<AudioBuffer>,
        report: Arc<VoiceReport>,
    },
    /// A recording was refused or ended; its report is already filled in.
    Recording { report: Arc<RecordingReport> },
}

#[cfg(test)]
mod tests {
    use super::voice::VoiceId;

    #[test]
    fn ids_are_minted_in_order() {
        let first = VoiceId::next();
        let second = VoiceId::next();
        assert!(first < second);
    }
}
