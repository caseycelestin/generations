//! Scaffolding shared by tests across the crate. Compiled only for tests,
//! so nothing here can be reached from the library.

use crate::ledger::{Ledger, Reservation};

/// A reservation that pays for anything, for tests of a function that
/// takes one but are not about the budget. Made on a throwaway ledger
/// with no limit; a reservation outlives its ledger.
pub(crate) fn paid() -> Reservation {
    Ledger::new(usize::MAX).reserve(usize::MAX).unwrap()
}
