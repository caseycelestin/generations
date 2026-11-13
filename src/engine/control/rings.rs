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
struct Rings {
    to_renderer: Producer<Command>,
    from_renderer: Consumer<Return>,
}

impl Rings {
    fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self {
            to_renderer,
            from_renderer,
        }
    }

    /// Collects, then pushes. A full ring hands the command back.
    fn send(&mut self, command: Command) -> Result<(), Command> {
        self.collect();
        self.to_renderer
            .push(command)
            .map_err(|PushError::Full(command)| command)
    }

    /// Frees everything handed back.
    fn collect(&mut self) {
        while let Ok(returned) = self.from_renderer.pop() {
            match returned {
                Return::Voice { audio, report } => drop((audio, report)),
                Return::Recording { report } => drop(report),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::RingBuffer;

    use super::*;
    use crate::engine::protocol::voice::{VoiceCommand, VoiceId, VoiceReport};
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
        assert!(rings.send(command()).is_ok());
        assert!(matches!(
            rings.send(command()),
            Err(ControlError::CommandsFull)
        ));
    }

    #[test]
    fn a_send_first_frees_what_came_back() {
        let (to_renderer, _from_control) = RingBuffer::new(1);
        let (mut to_control, from_renderer) = RingBuffer::new(1);
        let rings = SharedRings::new(to_renderer, from_renderer);
        let audio = Arc::new(buffer(ChannelLayout::Mono, &[0.0]));
        let returned = Return::Voice {
            audio: Arc::clone(&audio),
            report: Arc::new(VoiceReport::new()),
        };
        to_control.push(returned).unwrap();

        rings.send(command()).unwrap();

        assert_eq!(Arc::strong_count(&audio), 1);
        assert_eq!(to_control.slots(), 1);
    }
}
