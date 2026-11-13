//! The control side's ends of the two rings, kept together so every push
//! follows a drain. That is what lets [`return_capacity`] size the ring
//! back so it never fills.
//!
//! [`return_capacity`]: crate::engine::return_capacity

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer, PushError};

use super::ControlError;
use crate::engine::protocol::{Command, Return};

/// The rings of one renderer, shared by its `Control` and every handle it
/// makes. Clones share them.
#[derive(Debug, Clone)]
pub(crate) struct SharedRings(Arc<Mutex<Rings>>);

impl SharedRings {
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self(Arc::new(Mutex::new(Rings::new(to_renderer, from_renderer))))
    }

    pub(crate) fn send(&self, command: impl Into<Command>) -> Result<(), ControlError> {
        self.lock()
            .send(command.into())
            .map_err(|_| ControlError::CommandsFull)
    }

    pub(crate) fn collect(&self) {
        self.lock().collect();
    }

    /// A panic while locked leaves the rings intact, so a poisoned lock
    /// is taken anyway.
    fn lock(&self) -> MutexGuard<'_, Rings> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug)]
pub(crate) struct Rings {
    to_renderer: Producer<Command>,
    from_renderer: Consumer<Return>,
}

impl Rings {
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self {
            to_renderer,
            from_renderer,
        }
    }

    /// Collects, then pushes. A full ring hands the command back.
    pub(crate) fn send(&mut self, command: Command) -> Result<(), Command> {
        self.collect();
        self.to_renderer
            .push(command)
            .map_err(|PushError::Full(command)| command)
    }

    /// Frees everything handed back.
    pub(crate) fn collect(&mut self) {
        while let Ok(returned) = self.from_renderer.pop() {
            match returned {
                Return::Voice { audio, report } => drop((audio, report)),
            }
        }
    }
}
