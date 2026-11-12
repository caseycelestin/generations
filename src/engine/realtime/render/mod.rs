//! The audio thread: turns commands and audio into periods of output.

use rtrb::{Consumer, Producer};

use crate::engine::protocol::{Command, Return};

#[derive(Debug)]
pub struct Renderer {
    from_control: Consumer<Command>,
    to_control: Producer<Return>,
}

impl Renderer {
    pub(crate) fn new(from_control: Consumer<Command>, to_control: Producer<Return>) -> Self {
        Self {
            from_control,
            to_control,
        }
    }
}
