//! An input on the render side, while it is open: which capture channels
//! make its stereo frames. It holds no audio.

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputChannels, InputEnd, InputRequest};
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Input {
    permit: ReplyPermit,
    channels: InputChannels,
    /// Samples per device frame.
    capture_channels: usize,
}

impl Input {
    pub(crate) fn open(
        permit: ReplyPermit,
        channels: InputChannels,
        capture_channels: usize,
    ) -> Self {
        Self {
            permit,
            channels,
            capture_channels,
        }
    }

    /// Mono goes to both sides at full level.
    pub(crate) fn render(&self, input: &[Pcm], scratch: &mut [Frame]) {
        // Never zero: no input opens on a device without channels.
        let frames = input.chunks_exact(self.capture_channels);
        for (into, frame) in scratch.iter_mut().zip(frames) {
            *into = match self.channels {
                InputChannels::Mono(channel) => [frame[channel]; Renderer::OUTPUT_CHANNELS],
                InputChannels::Stereo(left, right) => [frame[left], frame[right]],
            };
        }
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
    use super::*;
    use crate::engine::realtime::capture::{Feeder, ring};

    fn input(channels: InputChannels) -> Input {
        Input::open(ReplyPermit::for_test(0), channels)
    }

    /// Inputs on a ring `channels` wide with room for `backlog` frames,
    /// and the ring's feeder.
    fn inputs(channels: usize, backlog: usize, open: &[InputChannels]) -> (Feeder, Inputs) {
        let (feeder, reader) = ring(channels, backlog);
        let mut inputs = Inputs::new(reader, open.len());
        for &channels in open {
            inputs.add(input(channels));
        }
        (feeder, inputs)
    }

    /// Reads a block of `frames`, returning what each input rendered.
    fn read(inputs: &mut Inputs, frames: usize) -> Vec<Vec<Frame>> {
        let mut into = vec![[0.9; 2]; frames];
        let mut heard = Vec::new();
        inputs.read(&mut into, |_, frames| heard.push(frames.to_vec()));
        heard
    }

    #[test]
    fn mono_is_rendered_to_both_sides_with_silence_after() {
        let device: [&[Pcm]; 2] = [&[0.1, 0.9], &[0.2, 0.8]];
        let mut into = [[0.5; 2]; 3];
        input(InputChannels::Mono(1)).render(device.into_iter(), &mut into);
        assert_eq!(into, [[0.9, 0.9], [0.8, 0.8], [0.0, 0.0]]);
    }

    #[test]
    fn stereo_takes_its_two_channels_left_then_right() {
        let device: [&[Pcm]; 1] = [&[0.1, 0.2, 0.3]];
        let mut into = [[0.0; 2]; 1];
        input(InputChannels::Stereo(2, 0)).render(device.into_iter(), &mut into);
        assert_eq!(into, [[0.3, 0.1]]);
    }

    #[test]
    fn read_takes_no_more_than_fits() {
        let (mut feeder, mut inputs) = inputs(1, 8, &[InputChannels::Mono(0)]);
        feeder.feed(&[0.1, 0.2, 0.3]);
        assert_eq!(read(&mut inputs, 2), [[[0.1, 0.1], [0.2, 0.2]]]);
        assert_eq!(read(&mut inputs, 2), [[[0.3, 0.3], [0.0, 0.0]]]);
    }

    #[test]
    fn read_takes_whole_frames_across_the_ring_wrap() {
        let (mut feeder, mut inputs) = inputs(2, 3, &[InputChannels::Stereo(0, 1)]);
        feeder.feed(&[0.1, 0.2, 0.3, 0.4]);
        read(&mut inputs, 2);
        // Lands on the ring's last frame and its first.
        feeder.feed(&[0.5, 0.6, 0.7, 0.8]);
        assert_eq!(read(&mut inputs, 2), [[[0.5, 0.6], [0.7, 0.8]]]);
    }

    #[test]
    fn every_open_input_reads_the_same_frames() {
        let open = [InputChannels::Mono(0), InputChannels::Mono(1)];
        let (mut feeder, mut inputs) = inputs(2, 4, &open);
        feeder.feed(&[0.1, 0.2]);
        assert_eq!(read(&mut inputs, 1), [[[0.1, 0.1]], [[0.2, 0.2]]]);
    }
}
