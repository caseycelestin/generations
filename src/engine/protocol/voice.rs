//! What crosses for voices: playings of a sample.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    /// Play `audio` from its first frame.
    Add {
        id: VoiceId,
        audio: Arc<AudioBuffer>,
        report: Arc<VoiceReport>,
    },
    /// Stop the voice, if it is still playing.
    Remove(VoiceId),
}

id! {
    /// One playing of a sample. The lowest id is the oldest; a stale
    /// remove names nothing and does nothing.
    VoiceId
}

/// Why a voice ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEnd {
    /// It played to its last frame.
    Finished,
    /// Its `stop` arrived.
    Stopped,
    /// It was the oldest when a new voice needed its place.
    Cut,
}

/// The audio thread's report on a voice: empty while it plays, then why it
/// ended. The handle reads it; the renderer writes it with one wait-free
/// store, so it shows the moment the voice ends.
///
/// When other features need reports, the box becomes a generic
/// `Report<T>` and a `reason!` macro generates each reason enum with its
/// `u8` conversions from the variant list, so a feature writes only that
/// list. A macro for the conversions because safe Rust cannot turn a `u8`
/// back into an enum without a `match` per type; a generic for the box
/// because it is the same for every reason.
#[derive(Debug)]
pub(crate) struct VoiceReport(AtomicU8);

impl VoiceReport {
    const PLAYING: u8 = 0;

    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(Self::PLAYING))
    }

    /// For the renderer, once, when the voice ends.
    pub(crate) fn end(&self, why: VoiceEnd) {
        let code = match why {
            VoiceEnd::Finished => 1,
            VoiceEnd::Stopped => 2,
            VoiceEnd::Cut => 3,
        };
        self.0.store(code, Ordering::Release);
    }

    /// `None` while the voice plays.
    pub(crate) fn ended(&self) -> Option<VoiceEnd> {
        match self.0.load(Ordering::Acquire) {
            1 => Some(VoiceEnd::Finished),
            2 => Some(VoiceEnd::Stopped),
            3 => Some(VoiceEnd::Cut),
            _ => None,
        }
    }
}
