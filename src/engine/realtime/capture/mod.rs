//! The capture thread: moves frames from the input device's callback onto
//! the ring the renderer reads.

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::Pcm;

/// The ring is whole frames, allocated here on the control side; the
/// reader goes to the renderer.
pub(crate) fn ring(channels: usize, backlog: usize) -> (Feeder, Reader) {
    let (producer, consumer) = RingBuffer::new(backlog * channels);
    let feeder = Feeder {
        ring: producer,
        channels,
    };
    let reader = Reader {
        ring: consumer,
        channels,
    };
    (feeder, reader)
}

#[derive(Debug)]
pub struct Feeder {
    ring: Producer<Pcm>,
    /// Only whole frames are pushed, so one never splits across a full
    /// ring.
    channels: usize,
}

impl Feeder {
    /// Interleaved frames, `channels` wide. What does not fit is dropped,
    /// whole frames from the end; returns the frames written.
    pub fn feed(&mut self, frames: &[Pcm]) -> usize {
        let width = self.channels;
        debug_assert_eq!(frames.len() % width, 0, "input is not whole frames");
        let free = self.ring.slots();
        let fits = frames.len().min(free - free % width);
        if let Ok(chunk) = self.ring.write_chunk_uninit(fits) {
            chunk.fill_from_iter(frames[..fits].iter().copied());
        }
        fits / width
    }
}

/// The renderer's end of the ring, with the width both ends share.
#[derive(Debug)]
pub(crate) struct Reader {
    pub(crate) ring: Consumer<Pcm>,
    pub(crate) channels: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_that_do_not_fit_are_dropped_whole() {
        let (mut feeder, mut reader) = ring(2, 2);
        assert_eq!(feeder.feed(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]), 2);
        assert_eq!(feeder.feed(&[0.7, 0.8]), 0);
        let chunk = reader.ring.read_chunk(4).unwrap();
        let (first, second) = chunk.as_slices();
        assert_eq!([first, second].concat(), [0.1, 0.2, 0.3, 0.4]);
    }
}
