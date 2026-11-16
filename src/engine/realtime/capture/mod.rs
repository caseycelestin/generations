//! The capture thread: moves frames from the input device's callback onto
//! the rings the renderer reads.

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::Pcm;
use crate::layout::ChannelLayout;

/// Capture periods a ring holds: room for the capture and audio clocks to
/// disagree before frames are dropped.
const RING_PERIODS: usize = 8;

#[derive(Debug)]
pub struct Feeder {
    frames: Producer<Pcm>,
    /// Only whole frames are pushed, so one never splits across a full
    /// ring.
    layout: ChannelLayout,
}

impl Feeder {
    /// The ring is whole frames, allocated here on the control side; the
    /// reading end goes to the renderer.
    pub(crate) fn new(layout: ChannelLayout, period: usize) -> (Self, Consumer<Pcm>) {
        let (frames, reader) = RingBuffer::new(period * RING_PERIODS * layout.count());
        (Self { frames, layout }, reader)
    }
}
