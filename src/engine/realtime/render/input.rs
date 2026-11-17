//! An input on the render side, while it is open.

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputChannels, InputEnd, InputRequest};
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
