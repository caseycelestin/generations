//! What crosses for voices: playings of audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    Add(VoiceSpec),
    /// Stop the voice, if it is still playing.
    Remove(VoiceId),
    RemoveAll,
    /// Stop every voice playing this audio.
    RemoveAudio(BufferAddress),
}

/// A voice as asked for: what to play and where to report. Fixed for the
/// voice's life; the renderer keeps it whole beside its playing state.
#[derive(Debug)]
pub(crate) struct VoiceSpec {
    pub(crate) id: VoiceId,
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) report: Arc<VoiceReport>,
}

id! {
    /// One playing of audio. The lowest id is the oldest; a stale
    /// remove names nothing and does nothing.
    VoiceId
}

/// Names audio to the renderer without sending it, so nothing comes back
/// to be freed. Sent while the caller still holds the audio, so the
/// address is still that audio's when the command is applied: a later
/// buffer at the same address is started by a play behind it on the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferAddress(usize);

impl BufferAddress {
    pub(crate) fn of(audio: &Arc<AudioBuffer>) -> Self {
        Self(Arc::as_ptr(audio) as usize)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_report_has_not_ended() {
        assert_eq!(VoiceReport::new().ended(), None);
    }

    #[test]
    fn each_ending_reads_back_as_itself() {
        for why in [VoiceEnd::Finished, VoiceEnd::Stopped, VoiceEnd::Cut] {
            let report = VoiceReport::new();
            report.end(why);
            assert_eq!(report.ended(), Some(why));
        }
    }
}
