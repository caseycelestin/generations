//! The control side's ends of the two rings to one renderer: commands go
//! out on one, what the renderer lets go of comes back on the other.
//! Transport only: [`Rings`] moves values and knows nothing of what they
//! mean, which handle sent them or what state the control side keeps
//! about the renderer.
//!
//! The two ends are kept together, and used under one lock by whoever
//! owns them, for one reason: every push follows a drain. Between two
//! drains the ring back can then receive at most what the renderer held
//! at the first one plus one thing per command waiting to be applied,
//! which is the arithmetic its capacity is sized from, so a push on the
//! render side never finds it full.

use rtrb::{Consumer, Producer, PushError};

use crate::engine::protocol::{Command, Return};

/// The control side's ends of the ring to the renderer and the ring back
/// from it. Has one owner, which locks it; never cloned, never shared.
#[derive(Debug)]
pub(crate) struct Rings {
    /// Commands, to the renderer.
    to_renderer: Producer<Command>,
    /// Everything the renderer lets go of, from it.
    from_renderer: Consumer<Return>,
}

impl Rings {
    /// Rings sending on `to_renderer` and collecting from `from_renderer`.
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self {
            to_renderer,
            from_renderer,
        }
    }

    /// Collects, then pushes `command`. A full ring hands it back.
    pub(crate) fn send(&mut self, command: Command) -> Result<(), Command> {
        self.collect();
        self.to_renderer
            .push(command)
            .map_err(|PushError::Full(command)| command)
    }

    /// Collects, then pushes every command of `commands`, or none of them
    /// if the ring has no room for all.
    pub(crate) fn send_all(
        &mut self,
        commands: impl ExactSizeIterator<Item = Command>,
    ) -> Result<(), Full> {
        self.collect();
        if self.to_renderer.slots() < commands.len() {
            return Err(Full);
        }
        for command in commands {
            self.to_renderer
                .push(command)
                .expect("room was checked, and this is the only producer");
        }
        Ok(())
    }

    /// Takes everything the renderer has handed back since the last
    /// collect, and drops it here, on the control side.
    pub(crate) fn collect(&mut self) {
        while self.from_renderer.pop().is_ok() {}
    }
}

/// The ring to the renderer had no room for every command of a
/// `send_all`; none was sent.
#[derive(Debug)]
pub(crate) struct Full;
