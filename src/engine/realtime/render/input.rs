//! An input on the render side, while it is open.

use rtrb::Consumer;

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputEnd, InputRequest};
use crate::layout::ChannelLayout;
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Input {
    permit: ReplyPermit,
    reader: Consumer<Pcm>,
    layout: ChannelLayout,
}

impl Input {
    pub(crate) fn open(permit: ReplyPermit, reader: Consumer<Pcm>, layout: ChannelLayout) -> Self {
        Self {
            permit,
            reader,
            layout,
        }
    }

    /// As many frames as have arrived, up to `into`'s length; silence after
    /// them, which is a gap when the capture side runs late. Mono goes to
    /// both sides at full level.
    pub(crate) fn read(&mut self, into: &mut [Frame]) {
        let width = self.layout.count();
        let frames = into.len().min(self.reader.slots() / width);
        let Ok(chunk) = self.reader.read_chunk(frames * width) else {
            into.fill([0.0; Renderer::CHANNELS]);
            return;
        };
        let (sound, silence) = into.split_at_mut(frames);
        silence.fill([0.0; Renderer::CHANNELS]);
        // The ring holds whole frames, so its two slices split between
        // frames, never inside one.
        let (first, second) = chunk.as_slices();
        match self.layout {
            ChannelLayout::Mono => {
                for (frame, &sample) in sound.iter_mut().zip(first.iter().chain(second)) {
                    *frame = [sample, sample];
                }
            }
            ChannelLayout::Stereo => {
                let (first, _) = first.as_chunks::<2>();
                let (second, _) = second.as_chunks::<2>();
                for (frame, pair) in sound.iter_mut().zip(first.iter().chain(second)) {
                    *frame = *pair;
                }
            }
        }
        chunk.commit_all();
    }

    pub(crate) fn feeder_gone(&self) -> bool {
        self.reader.is_abandoned()
    }

    pub(crate) fn is(&self, id: RequestId) -> bool {
        self.permit.id() == id
    }

    /// The one way an input leaves: its reader goes back in the reply.
    pub(crate) fn end(self, why: InputEnd, requests: &mut RealtimeEnd<InputRequest>) {
        requests.close(self.permit, (why, self.reader));
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
