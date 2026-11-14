//! The control side's ends of the two rings, kept together so every push
//! follows a drain. That is what lets [`return_capacity`] size the ring
//! back so it never fills.
//!
//! [`return_capacity`]: crate::engine::return_capacity

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};

use super::ControlError;
use super::handle::Inbox;
use crate::engine::protocol::{Command, Event, HandleId, Message};

/// One per renderer, shared by its `Control` and every handle.
#[derive(Debug, Clone)]
pub(crate) struct SharedRings(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    rings: Mutex<Rings>,
    /// Outside the lock, so a handle can mint while holding a permit.
    ids: AtomicU64,
}

impl SharedRings {
    pub(crate) fn new(
        to_renderer: Producer<Message<Command>>,
        from_renderer: Consumer<Message<Event>>,
    ) -> Self {
        Self(Arc::new(Shared {
            rings: Mutex::new(Rings::new(to_renderer, from_renderer)),
            ids: AtomicU64::new(0),
        }))
    }

    pub(crate) fn send(
        &self,
        id: HandleId,
        command: impl Into<Command>,
    ) -> Result<(), ControlError> {
        self.reserve()?.send(id, command);
        Ok(())
    }

    /// Collects, then holds a free slot, so a caller can give up what it
    /// sends only once the send cannot fail.
    pub(crate) fn reserve(&self) -> Result<Permit<'_>, ControlError> {
        let mut rings = self.lock();
        rings.collect();
        if rings.to_renderer.is_full() {
            return Err(ControlError::CommandsFull);
        }
        Ok(Permit(rings))
    }

    pub(crate) fn collect(&self) {
        self.lock().collect();
    }

    /// Only uniqueness matters, so no ordering with other memory.
    pub(crate) fn next_id(&self) -> HandleId {
        HandleId(self.0.ids.fetch_add(1, Ordering::Relaxed))
    }

    /// A panic while locked leaves the rings intact, so a poisoned lock
    /// is taken anyway.
    fn lock(&self) -> MutexGuard<'_, Rings> {
        self.0.rings.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A free slot on the ring to the renderer. Holds the lock, and only the
/// control side pushes, so the slot stays free until `send`.
pub(crate) struct Permit<'a>(MutexGuard<'a, Rings>);

impl Permit<'_> {
    /// Before the send, under the same lock, so no return for `id` can be
    /// collected before its place exists.
    pub(crate) fn register(&mut self, id: HandleId, inbox: Arc<Mutex<dyn Inbox>>) {
        let free = self.0.handles.iter_mut().find(|place| place.is_none());
        debug_assert!(free.is_some(), "sized for every id still owed a return");
        if let Some(place) = free {
            *place = Some((id, inbox));
        }
    }

    pub(crate) fn send(mut self, id: HandleId, command: impl Into<Command>) {
        let message = Message {
            id,
            data: command.into(),
        };
        let pushed = self.0.to_renderer.push(message);
        debug_assert!(pushed.is_ok(), "a permit holds a free slot");
    }
}

#[derive(Debug)]
struct Rings {
    to_renderer: Producer<Message<Command>>,
    from_renderer: Consumer<Message<Event>>,
    /// Which handle's inbox an id's returns go to, held until its last
    /// return arrives. Every id held here still has that return coming, so
    /// there are never more than the ring back can hold, and the list takes
    /// its size.
    handles: Box<[Record]>,
}

type Record = Option<(HandleId, Arc<Mutex<dyn Inbox>>)>;

impl Rings {
    fn new(
        to_renderer: Producer<Message<Command>>,
        from_renderer: Consumer<Message<Event>>,
    ) -> Self {
        let handles = (0..from_renderer.buffer().capacity())
            .map(|_| None)
            .collect();
        Self {
            to_renderer,
            from_renderer,
            handles,
        }
    }

    fn collect(&mut self) {
        while let Ok(Message { id, data }) = self.from_renderer.pop() {
            self.deliver(id, data);
        }
    }

    /// Hands `event` to its id's inbox, and frees the record once the inbox
    /// expects nothing more. An event for an id nobody registered is
    /// dropped here.
    fn deliver(&mut self, id: HandleId, event: Event) {
        let record = self
            .handles
            .iter_mut()
            .find(|record| matches!(record, Some((held, _)) if *held == id));
        let Some(record) = record else {
            return;
        };
        let Some((_, inbox)) = record else {
            return;
        };
        let done = inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .deliver(event);
        if done {
            *record = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::RingBuffer;

    use super::*;
    use crate::engine::protocol::voice::{
        VoiceCommand, VoiceEnd, VoiceEvent, VoiceId, VoiceSpec, VoiceState,
    };
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    fn command() -> Command {
        VoiceCommand::Remove(VoiceId::next()).into()
    }

    #[test]
    fn a_full_ring_refuses_the_command() {
        let (to_renderer, _from_control) = RingBuffer::new(1);
        let (_to_control, from_renderer) = RingBuffer::new(1);
        let rings = SharedRings::new(to_renderer, from_renderer);
        assert!(rings.send(HandleId(0), command()).is_ok());
        assert!(matches!(
            rings.send(HandleId(0), command()),
            Err(ControlError::CommandsFull)
        ));
    }

    #[test]
    fn a_send_first_frees_what_came_back() {
        let (to_renderer, _from_control) = RingBuffer::new(1);
        let (mut to_control, from_renderer) = RingBuffer::new(1);
        let rings = SharedRings::new(to_renderer, from_renderer);
        let audio = Arc::new(buffer(ChannelLayout::Mono, &[0.0]));
        let spec = VoiceSpec {
            id: VoiceId::next(),
            audio: Arc::clone(&audio),
        };
        let returned = Message {
            id: HandleId(0),
            data: VoiceEvent {
                state: VoiceState::Ended(VoiceEnd::Finished),
                spec: Some(spec),
            }
            .into(),
        };
        to_control.push(returned).unwrap();

        rings.send(HandleId(0), command()).unwrap();

        assert_eq!(Arc::strong_count(&audio), 1);
        assert_eq!(to_control.slots(), 1);
    }
}
