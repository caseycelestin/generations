//! An input on the render side, while it is open.

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputChannels, InputEnd, InputRequest};
use crate::engine::realtime::capture::Reader;
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Input {
    permit: ReplyPermit,
    channels: InputChannels,
}

impl Input {
    pub(crate) fn open(permit: ReplyPermit, channels: InputChannels) -> Self {
        Self { permit, channels }
    }

    /// One item per device frame, up to `into`'s length; silence after
    /// them, which is a gap when the capture side runs late. Mono goes to
    /// both sides at full level.
    pub(crate) fn read<'a>(&self, frames: impl Iterator<Item = &'a [Pcm]>, into: &mut [Frame]) {
        let mut read = 0;
        for (frame, device) in into.iter_mut().zip(frames) {
            *frame = match self.channels {
                InputChannels::Mono(channel) => [device[channel]; Renderer::CHANNELS],
                InputChannels::Stereo(left, right) => [device[left], device[right]],
            };
            read += 1;
        }
        into[read..].fill([0.0; Renderer::CHANNELS]);
    }

    pub(crate) fn is(&self, id: RequestId) -> bool {
        self.permit.id() == id
    }

    /// The one way an input leaves.
    pub(crate) fn end(self, why: InputEnd, requests: &mut RealtimeEnd<InputRequest>) {
        requests.close(self.permit, why);
    }
}

/// One capture device's ring, and the inputs reading it.
#[derive(Debug)]
pub(crate) struct Inputs {
    capture: Reader,
    /// Never past its capacity, so adding one never allocates.
    open: Vec<Input>,
}

impl Inputs {
    pub(crate) fn new(capture: Reader, count: usize) -> Self {
        Self {
            capture,
            open: Vec::with_capacity(count),
        }
    }

    /// Up to `into`'s length in whole frames off the ring; each open input
    /// copies its channels into `into` in turn, then `each` uses them.
    pub(crate) fn read(&mut self, into: &mut [Frame], mut each: impl FnMut(&Input, &mut [Frame])) {
        let width = self.capture.channels;
        let frames = into.len().min(self.capture.ring.slots() / width);
        let chunk = self.capture.ring.read_chunk(frames * width);
        // A chunk that cannot be read is no frames, so every input still
        // reads, as silence. The ring holds whole frames, so its two slices
        // split between frames, never inside one.
        let (first, second) = match &chunk {
            Ok(chunk) => chunk.as_slices(),
            Err(_) => (&[][..], &[][..]),
        };
        for input in &self.open {
            let device = first.chunks_exact(width).chain(second.chunks_exact(width));
            input.read(device, into);
            each(input, into);
        }
        if let Ok(chunk) = chunk {
            chunk.commit_all();
        }
    }

    pub(crate) fn add(&mut self, input: Input) {
        let room = self.open.len() < self.open.capacity();
        debug_assert!(room, "one request per input");
        self.open.push(input);
    }

    pub(crate) fn holds(&self, id: RequestId) -> bool {
        self.open.iter().any(|input| input.is(id))
    }

    pub(crate) fn remove(&mut self, id: RequestId) -> Option<Input> {
        let at = self.open.iter().position(|input| input.is(id))?;
        Some(self.open.swap_remove(at))
    }

    pub(crate) fn feeder_gone(&self) -> bool {
        self.capture.ring.is_abandoned()
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = Input> + '_ {
        self.open.drain(..)
    }
}

#[cfg(test)]
mod tests {
    use rtrb::{Producer, RingBuffer};

    use super::*;

    /// An input on a ring of `capacity` samples, and the ring's writing end.
    fn input(layout: ChannelLayout, capacity: usize) -> (Producer<Pcm>, Input) {
        let (feeder, reader) = RingBuffer::new(capacity);
        (
            feeder,
            Input::open(ReplyPermit::for_test(0), reader, layout),
        )
    }

    fn push(feeder: &mut Producer<Pcm>, samples: &[Pcm]) {
        let chunk = feeder.write_chunk_uninit(samples.len()).unwrap();
        chunk.fill_from_iter(samples.iter().copied());
    }

    #[test]
    fn mono_is_read_to_both_sides_with_silence_after() {
        let (mut feeder, mut input) = input(ChannelLayout::Mono, 8);
        push(&mut feeder, &[0.1, 0.2]);
        let mut into = [[0.9; 2]; 3];
        input.read(&mut into);
        assert_eq!(into, [[0.1, 0.1], [0.2, 0.2], [0.0, 0.0]]);
    }

    #[test]
    fn read_takes_no_more_than_fits() {
        let (mut feeder, mut input) = input(ChannelLayout::Mono, 8);
        push(&mut feeder, &[0.1, 0.2, 0.3]);
        let mut into = [[0.0; 2]; 2];
        input.read(&mut into);
        input.read(&mut into);
        assert_eq!(into, [[0.3, 0.3], [0.0, 0.0]]);
    }

    #[test]
    fn stereo_is_read_in_whole_frames_across_the_ring_wrap() {
        let (mut feeder, mut input) = input(ChannelLayout::Stereo, 6);
        let mut into = [[0.0; 2]; 2];
        push(&mut feeder, &[0.1, 0.2, 0.3, 0.4]);
        input.read(&mut into);
        // Lands on the ring's last frame and its first.
        push(&mut feeder, &[0.5, 0.6, 0.7, 0.8]);
        input.read(&mut into);
        assert_eq!(into, [[0.5, 0.6], [0.7, 0.8]]);
    }
}
