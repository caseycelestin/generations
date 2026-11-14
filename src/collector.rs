//! Freeing off the real-time thread: it gives up what it is done with, and
//! a control thread drops it later. Knows nothing of ids or replies; what
//! someone wants back is a reply, not garbage.
//!
//! To come: one ring of the values given up, sized at creation. Giving up
//! never blocks, allocates or fails, provided the ring holds the most the
//! real-time thread can hold and the control thread collects before each
//! send. The caller sizes it and keeps that rule.

use std::marker::PhantomData;

use thiserror::Error;

/// A refused value, handed back: the ring was full, so it was sized too
/// small or not collected in time.
#[derive(Debug, Error)]
#[error("the collector is full")]
pub struct Full<T>(pub T);

/// `capacity` is in values: the most the real-time side can hold at once.
pub fn new<T>(capacity: usize) -> (Collector<T>, Disposer<T>) {
    todo!()
}

/// The control side's end.
#[derive(Debug)]
pub struct Collector<T>(PhantomData<T>);

/// Clones share one collector, so any control-side caller can collect.
/// Written out, as a derive would demand `T` be `Clone`.
impl<T> Clone for Collector<T> {
    fn clone(&self) -> Self {
        todo!()
    }
}

impl<T> Collector<T> {
    /// Drops everything given up so far.
    pub fn collect(&self) {
        todo!()
    }
}

/// The real-time side's end.
#[derive(Debug)]
pub struct Disposer<T>(PhantomData<T>);

impl<T> Disposer<T> {
    pub fn dispose(&mut self, value: T) -> Result<(), Full<T>> {
        todo!()
    }
}
