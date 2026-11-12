//! The control side's ends of the two rings, kept together so every push
//! follows a drain. That is what lets [`return_capacity`] size the ring
//! back so it never fills.
//!
//! [`return_capacity`]: crate::engine::return_capacity

use rtrb::{Consumer, Producer, PushError};

use crate::engine::protocol::{Command, Return};

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

    /// Collects, then pushes all of `commands` or none.
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

    /// Drops everything handed back.
    pub(crate) fn collect(&mut self) {
        while self.from_renderer.pop().is_ok() {}
    }
}

/// No room for the whole batch; nothing was sent.
#[derive(Debug)]
pub(crate) struct Full;
