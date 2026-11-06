//! The engine's memory budget, kept as one figure.
//!
//! The [`Ledger`] is a quota, not an allocator. It holds the budget the
//! engine was opened with and the bytes currently promised against it,
//! and nothing else: it never owns a buffer and never hands out a pointer.
//! A client asks for so many bytes and gets a [`Reservation`] or a refusal,
//! then allocates on the ordinary heap; the operating system's reservation
//! for the engine's process is still the real guarantee that the memory
//! exists. What the ledger adds is the other half of that arrangement: the
//! engine never promises more than the figure, so the deployment only has
//! to cover one number.
//!
//! The only comparison against the budget is inside [`Ledger::reserve`].
//! A reservation is held next to the buffer it paid for and gives the
//! bytes back when it is dropped, so release is automatic and cannot be
//! forgotten or counted twice. Every large audio allocation in the crate
//! asks here first; one that does not makes the figure a lie.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use thiserror::Error;

/// The bytes asked for would put the engine's memory over its budget.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "{requested} bytes would put the engine's memory at {} of a {budget} byte budget",
    in_use + requested
)]
pub struct OverBudget {
    /// Bytes the refused reservation would have cost.
    pub requested: usize,
    /// Bytes reserved at the time.
    pub in_use: usize,
    /// The budget the engine was opened with.
    pub budget: usize,
}

/// What the ledger and its reservations share: the figure and the count.
#[derive(Debug)]
struct Account {
    budget: usize,
    /// Bytes promised to live reservations. It protects nothing but
    /// itself, so relaxed ordering is enough.
    in_use: AtomicUsize,
}

/// The engine's memory budget and how much of it is spoken for.
#[derive(Debug)]
pub struct Ledger {
    account: Arc<Account>,
}

impl Ledger {
    /// A ledger over `budget` bytes, none of them in use.
    pub fn new(budget: usize) -> Self {
        Self {
            account: Arc::new(Account {
                budget,
                in_use: AtomicUsize::new(0),
            }),
        }
    }

    /// Reserves `bytes` against the budget.
    ///
    /// Refused, with nothing changed, if the result would exceed the
    /// budget. Reserving exactly up to it is allowed. The bytes stay
    /// reserved until the returned [`Reservation`] is dropped.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation, OverBudget> {
        let account = &self.account;
        account
            .in_use
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                in_use
                    .checked_add(bytes)
                    .filter(|&total| total <= account.budget)
            })
            .map(|_| Reservation {
                bytes,
                account: Arc::clone(account),
            })
            .map_err(|in_use| OverBudget {
                requested: bytes,
                in_use,
                budget: account.budget,
            })
    }

    /// Bytes reserved right now. A front panel will want this; until then
    /// only tests ask.
    #[cfg(test)]
    pub fn in_use(&self) -> usize {
        self.account.in_use.load(Ordering::Relaxed)
    }
}

/// A promise of so many bytes, held next to the buffer that uses them.
///
/// Dropping it gives the bytes back. There is no other way to release
/// them, so a reservation cannot be released twice or forgotten.
#[derive(Debug)]
pub struct Reservation {
    bytes: usize,
    account: Arc<Account>,
}

impl Reservation {
    /// The bytes this reservation holds.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.account.in_use.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_counts_until_it_is_dropped() {
        let ledger = Ledger::new(100);
        assert_eq!(ledger.in_use(), 0);
        let a = ledger.reserve(30).unwrap();
        let b = ledger.reserve(20).unwrap();
        assert_eq!(a.bytes(), 30);
        assert_eq!(ledger.in_use(), 50);
        drop(a);
        assert_eq!(ledger.in_use(), 20);
        drop(b);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn over_budget_is_refused_and_changes_nothing() {
        let ledger = Ledger::new(50);
        let _a = ledger.reserve(40).unwrap();
        let err = ledger.reserve(20).unwrap_err();
        assert_eq!(
            err,
            OverBudget {
                requested: 20,
                in_use: 40,
                budget: 50,
            }
        );
        assert_eq!(ledger.in_use(), 40);
    }

    #[test]
    fn exactly_the_budget_is_allowed() {
        let ledger = Ledger::new(40);
        let _a = ledger.reserve(40).unwrap();
        assert_eq!(ledger.in_use(), 40);
        assert!(ledger.reserve(1).is_err());
    }

    #[test]
    fn released_bytes_can_be_reserved_again() {
        let ledger = Ledger::new(40);
        let a = ledger.reserve(40).unwrap();
        assert!(ledger.reserve(1).is_err());
        drop(a);
        assert!(ledger.reserve(40).is_ok());
    }

    #[test]
    fn a_reservation_outlives_the_ledger() {
        let ledger = Ledger::new(40);
        let a = ledger.reserve(40).unwrap();
        drop(ledger);
        drop(a);
    }

    #[test]
    fn an_unbounded_budget_does_not_overflow() {
        let ledger = Ledger::new(usize::MAX);
        let _a = ledger.reserve(usize::MAX - 1).unwrap();
        assert!(ledger.reserve(2).is_err());
    }
}
