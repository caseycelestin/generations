//! What crosses for recordings: the main out written into audio.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug)]
pub(crate) enum RecordingCommand {
    Start(Arc<RecordingReport>),
    Stop,
}

/// The audio thread's report on a recording: empty while it records, then
/// why it ended. A copy of `VoiceReport` until reports are made generic.
#[derive(Debug)]
pub(crate) struct RecordingReport(AtomicU8);

impl RecordingReport {
    const RECORDING: u8 = 0;

    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(Self::RECORDING))
    }

    /// For the renderer, once, when the recording ends.
    pub(crate) fn end(&self, why: RecordingEnd) {
        let code = match why {
            RecordingEnd::Full => 1,
            RecordingEnd::Stopped => 2,
        };
        self.0.store(code, Ordering::Release);
    }

    /// `None` while it records.
    pub(crate) fn ended(&self) -> Option<RecordingEnd> {
        match self.0.load(Ordering::Acquire) {
            1 => Some(RecordingEnd::Full),
            2 => Some(RecordingEnd::Stopped),
            _ => None,
        }
    }
}

/// Why a recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingEnd {
    /// Its audio has no room left.
    Full,
    /// Its `stop` arrived.
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_report_has_not_ended() {
        assert_eq!(RecordingReport::new().ended(), None);
    }

    #[test]
    fn each_ending_reads_back_as_itself() {
        for why in [RecordingEnd::Full, RecordingEnd::Stopped] {
            let report = RecordingReport::new();
            report.end(why);
            assert_eq!(report.ended(), Some(why));
        }
    }
}
