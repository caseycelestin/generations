//! The control side: runs on UI or event threads. May lock and allocate.

mod rings;
mod voice;

pub use voice::Voice;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::engine::protocol::{Command, Return};
use rings::Rings;

#[derive(Debug, Error)]
pub enum ControlError {
    /// The renderer has not caught up; the command was dropped.
    #[error("the command ring is full")]
    CommandsFull,
}

/// The control side's view of one renderer. Clones share it.
#[derive(Debug, Clone)]
pub struct Control(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    rings: Mutex<Rings>,
}

impl Shared {
    /// A panic while locked leaves the rings intact, so a poisoned lock
    /// is taken anyway.
    fn rings(&self) -> MutexGuard<'_, Rings> {
        self.rings.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Control {
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self(Arc::new(Shared {
            rings: Mutex::new(Rings::new(to_renderer, from_renderer)),
        }))
    }

    pub(crate) fn send(&self, command: Command) -> Result<(), ControlError> {
        self.0
            .rings()
            .send(command)
            .map_err(|_| ControlError::CommandsFull)
    }

    /// Frees what the renderer has handed back, returning its memory to
    /// the ledger. Every send does this first; call it when the app goes
    /// quiet, or before a load that needs the memory.
    pub fn collect(&self) {
        self.0.rings().collect();
    }
}
