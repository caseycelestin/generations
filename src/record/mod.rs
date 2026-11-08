//! Recording: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this will turn periods of input into
//! buffers the engine can store. The [`Recorder`] is the control side: the
//! engine mints it and the caller keeps it, and it will mint takes against
//! the engine's budget, each a buffer reserved and allocated here, on the
//! control thread, before it goes anywhere near the frames. The audio
//! side, the take itself, is not here yet.
//!
//! The recorder reserves through its own handle on the engine's ledger
//! rather than through the engine. The ledger is one figure however many
//! hands hold it, and the only comparison against the budget is still the
//! one inside its reserve, so this adds a holder and not a rule.

use crate::ledger::Ledger;

/// The control side of recording: mints takes against the engine's
/// budget and receives them back when they are done.
///
/// Minted by the engine, kept by the caller. It owns no audio and never
/// goes to the audio thread.
#[derive(Debug)]
pub struct Recorder {
    /// The engine's budget, shared. Every take's buffer is reserved here
    /// before it is allocated.
    // Read once takes are minted; until then the recorder only holds it.
    #[allow(dead_code)]
    ledger: Ledger,
}

impl Recorder {
    /// A recorder that reserves against `ledger`.
    pub(crate) fn new(ledger: Ledger) -> Self {
        Self { ledger }
    }

    /// The ledger this recorder reserves against, so a test can check it
    /// is the engine's.
    #[cfg(test)]
    pub(crate) fn ledger(&self) -> &Ledger {
        &self.ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorder_reserves_against_the_ledger_it_was_given() {
        let ledger = Ledger::new(100);
        let r = Recorder::new(ledger.clone());
        let held = r.ledger().reserve(60).unwrap();
        assert_eq!(ledger.in_use(), 60);
        assert!(ledger.reserve(41).is_err());
        drop(held);
        assert_eq!(ledger.in_use(), 0);
    }
}
