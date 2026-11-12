//! The audio thread: turns commands and resident audio into periods of
//! output.

use rtrb::{Consumer, Producer};

use crate::engine::protocol::{Command, Return};

/// The render side of one engine: holds its ends of the ring from the
/// control side and the ring back to it.
#[derive(Debug)]
pub struct Renderer {
    /// Commands, from the control side.
    from_control: Consumer<Command>,
    /// Everything handed back, to the control side.
    to_control: Producer<Return>,
}

impl Renderer {
    /// A renderer taking commands from `from_control` and handing back
    /// on `to_control`.
    pub(crate) fn new(from_control: Consumer<Command>, to_control: Producer<Return>) -> Self {
        Self {
            from_control,
            to_control,
        }
    }
}
