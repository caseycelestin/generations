//! What every handle shares; each feature's handle wraps one and keeps its
//! own rules beside it.

use std::fmt::Debug;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::ControlError;
use super::rings::SharedRings;
use crate::engine::protocol::{Command, Event, HandleId};

/// What a handle keeps of what the renderer says about it; each feature
/// decides what to keep. `Send` because the rings that fill it cross threads.
pub(crate) trait Inbox: Send + Debug {
    /// Returns true once nothing more will come, so the rings forget the id.
    fn deliver(&mut self, event: Event) -> bool;
}

#[derive(Debug)]
pub(crate) struct Handle<E> {
    rings: SharedRings,
    id: HandleId,
    /// Filled by `collect` with what the renderer said about this handle.
    inbox: Arc<Mutex<E>>,
}

impl<E: Inbox + Default + 'static> Handle<E> {
    /// Sends `command(spec)` to bring the handle's thing to the renderer,
    /// or hands `spec` back on a full ring. The inbox starts at its
    /// default: what the handle shows before the renderer has said anything.
    pub(crate) fn open<S, C: Into<Command>>(
        rings: &SharedRings,
        spec: S,
        command: impl FnOnce(S) -> C,
    ) -> Result<Self, S> {
        let Ok(mut permit) = rings.reserve() else {
            return Err(spec);
        };
        let id = rings.next_id();
        let inbox = Arc::new(Mutex::new(E::default()));
        permit.register(id, inbox.clone());
        permit.send(id, command(spec));
        Ok(Self {
            rings: rings.clone(),
            id,
            inbox,
        })
    }
}

impl<E> Handle<E> {
    pub(crate) fn send(&self, command: impl Into<Command>) -> Result<(), ControlError> {
        self.rings.send(self.id, command)
    }

    /// Collects first, so it holds what the renderer last said. A panic
    /// while locked leaves a whole value, so a poisoned lock is taken anyway.
    pub(crate) fn inbox(&self) -> MutexGuard<'_, E> {
        self.rings.collect();
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
