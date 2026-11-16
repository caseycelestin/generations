//! The capture thread: moves frames from the input device's callback onto
//! the rings the renderer reads.

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::Pcm;
use crate::layout::ChannelLayout;

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
    pub(crate) fn new(layout: ChannelLayout, backlog: usize) -> (Self, Consumer<Pcm>) {
        let (frames, reader) = RingBuffer::new(backlog * layout.count());
        (Self { frames, layout }, reader)
    }

    /// Interleaved frames of the input's layout. What does not fit is
    /// dropped, whole frames from the end; returns the frames written.
    pub fn feed(&mut self, frames: &[Pcm]) -> usize {
        let width = self.layout.count();
        debug_assert_eq!(frames.len() % width, 0, "input is not whole frames");
        let free = self.frames.slots();
        let fits = frames.len().min(free - free % width);
        if let Ok(chunk) = self.frames.write_chunk_uninit(fits) {
            chunk.fill_from_iter(frames[..fits].iter().copied());
        }
        fits / width
    }
}
