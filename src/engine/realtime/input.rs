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

    fn input(channels: InputChannels, capture_channels: usize) -> Input {
        Input::open(ReplyPermit::for_test(0), channels, capture_channels)
    }

    #[test]
    fn mono_is_rendered_to_both_sides() {
        let capture = [0.1, 0.9, 0.2, 0.8];
        let mut scratch = [[0.5; 2]; 2];
        input(InputChannels::Mono(1), 2).render(&capture, &mut scratch);
        assert_eq!(scratch, [[0.9, 0.9], [0.8, 0.8]]);
    }

    #[test]
    fn stereo_takes_its_two_channels_left_then_right() {
        let capture = [0.1, 0.2, 0.3];
        let mut scratch = [[0.0; 2]; 1];
        input(InputChannels::Stereo(2, 0), 3).render(&capture, &mut scratch);
        assert_eq!(scratch, [[0.3, 0.1]]);
    }
}
