//! Freeing off the real-time thread: it sends what it is done with, and a
//! control thread drops it later. Knows nothing of ids or replies; what
//! someone wants back is a reply, not garbage.
//!
//! One ring of the values sent, sized at creation. Sending never blocks or
//! allocates, and is refused only when the ring is full; it never is if the
//! ring holds the most the real-time side can hold and the control side
//! collects before handing it anything new. The caller sizes it and keeps
//! that rule.

use std::sync::{Arc, Mutex, PoisonError};

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use thiserror::Error;

/// A refused value, handed back: the ring was full, so it was sized too
/// small or not collected in time.
#[derive(Debug, Error)]
#[error("the collector is full")]
pub struct Full<T>(pub T);

/// `capacity` is in values: the most the real-time side can hold at once.
///
/// # Panics
/// If `capacity` is zero: a collector with no room refuses every value.
pub fn channel<T>(capacity: usize) -> (ControlEnd<T>, RealtimeEnd<T>) {
    assert!(capacity > 0, "capacity must not be zero");
    let (producer, consumer) = RingBuffer::new(capacity);
    let collector = ControlEnd(Arc::new(Mutex::new(ControlConsumer(consumer))));
    (collector, RealtimeEnd(producer))
}

/// The lock is for its clones; the real-time side never takes it. The last
/// clone collects as it drops; what is sent after that is freed when the
/// [`RealtimeEnd`] drops.
#[derive(Debug)]
pub struct ControlEnd<T>(Arc<Mutex<ControlConsumer<T>>>);

/// Clones share one collector, so any control-side caller can collect.
/// Written out, as a derive would demand `T` be `Clone`.
impl<T> Clone for ControlEnd<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> ControlEnd<T> {
    /// Drops everything given up so far.
    pub fn collect(&self) {
        // A panic while locked leaves the ring whole, so a poisoned lock is
        // taken anyway.
        let mut consumer = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        consumer.drain();
    }
}

/// What the control ends share, so dropping the last of them can collect.
#[derive(Debug)]
struct ControlConsumer<T>(Consumer<T>);

impl<T> ControlConsumer<T> {
    fn drain(&mut self) {
        while self.0.pop().is_ok() {}
    }
}

impl<T> Drop for ControlConsumer<T> {
    fn drop(&mut self) {
        self.drain();
    }
}

#[derive(Debug)]
pub struct RealtimeEnd<T>(Producer<T>);

impl<T> RealtimeEnd<T> {
    /// Queues `value` for the collector to drop.
    pub fn try_send(&mut self, value: T) -> Result<(), Full<T>> {
        self.0
            .push(value)
            .map_err(|PushError::Full(value)| Full(value))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn collect_drops_what_was_sent() {
        let (collector, mut sender) = channel(1);
        let value = Arc::new(0);
        sender.try_send(Arc::clone(&value)).unwrap();
        assert_eq!(Arc::strong_count(&value), 2);

        collector.collect();

        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    fn a_clone_collects_the_same_ring() {
        let (collector, mut sender) = channel(1);
        let value = Arc::new(0);
        sender.try_send(Arc::clone(&value)).unwrap();

        collector.clone().collect();

        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    fn a_full_ring_hands_the_value_back() {
        let (_collector, mut sender) = channel(1);
        sender.try_send(1).unwrap();
        assert!(matches!(sender.try_send(2), Err(Full(2))));
    }

    #[test]
    fn dropping_the_last_collector_drops_what_waits() {
        let (collector, mut sender) = channel(1);
        let clone = collector.clone();
        let value = Arc::new(0);
        sender.try_send(Arc::clone(&value)).unwrap();

        drop(clone);
        assert_eq!(Arc::strong_count(&value), 2);
        drop(collector);

        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    fn what_is_sent_after_the_control_end_is_gone_drops_with_the_realtime_end() {
        let (collector, mut sender) = channel(1);
        drop(collector);
        let value = Arc::new(0);
        sender.try_send(Arc::clone(&value)).unwrap();
        assert_eq!(Arc::strong_count(&value), 2);

        drop(sender);

        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    #[should_panic(expected = "capacity must not be zero")]
    fn a_zero_capacity_panics() {
        let _ = channel::<()>(0);
    }
}
