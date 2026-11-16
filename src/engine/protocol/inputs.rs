//! What crosses for inputs.

use rtrb::Consumer;

use crate::config::Pcm;
use crate::layout::ChannelLayout;
use crate::request::Request;

#[derive(Debug)]
pub(crate) struct InputRequest {
    pub(crate) reader: Consumer<Pcm>,
    pub(crate) layout: ChannelLayout,
}

/// The reader comes back with why it ended, so the ring is freed on the
/// control side.
impl Request for InputRequest {
    type Reply = (InputEnd, Consumer<Pcm>);
    type Status = InputStatus;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStatus {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEnd {
    /// Its `stop` arrived, or its handle was dropped.
    Stopped,
    /// Its feeder was dropped.
    FeederGone,
}
