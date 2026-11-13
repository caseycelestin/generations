//! What crosses for voices: playings of audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::audio::AudioBuffer;

use super::BufferAddress;

#[derive(Debug)]
pub(crate) enum VoiceCommand {
    Add(VoiceSpec),
    /// A stale id names nothing and does nothing.
    Remove(VoiceId),
    /// Every voice playing this audio.
    RemoveAudio(BufferAddress),
    RemoveAll,
}

/// Fixed for the voice's life; the renderer keeps it whole beside its
/// playing state, to hand back.
#[derive(Debug)]
pub(crate) struct VoiceSpec {
    pub(crate) id: VoiceId,
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) report: Arc<VoiceReport>,
}

id! {
    /// Lower is older, which is how the renderer finds the voice to cut.
    VoiceId
}

/// Empty while the voice plays, then why it ended.
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

    pub(crate) fn ended(&self) -> Option<VoiceEnd> {
        match self.0.load(Ordering::Acquire) {
            1 => Some(VoiceEnd::Finished),
            2 => Some(VoiceEnd::Stopped),
            3 => Some(VoiceEnd::Cut),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEnd {
    /// It played to its last frame.
    Finished,
    /// Its `stop` arrived.
    Stopped,
    /// It was the oldest when a new voice needed its place.
    Cut,
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
