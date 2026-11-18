//! An input on the render side, while it is open: which capture channels
//! make its stereo frames. It holds no audio.

use std::sync::atomic::Ordering;

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputChannels, InputData};
use crate::transfer::{self, Completion};

#[derive(Debug)]
pub(crate) struct Input {
    data: Completion<InputData>,
    /// Samples per capture frame.
    capture_channels: usize,
}

impl Input {
    fn open(data: Completion<InputData>, capture_channels: usize) -> Self {
        Self {
            data,
            capture_channels,
        }
    }

    /// Mono goes to both sides at full level.
    pub(crate) fn render(&self, input: &[Pcm], scratch: &mut [Frame]) {
        // Never zero: no input opens on a device without channels.
        let frames = input.chunks_exact(self.capture_channels);
        for (into, frame) in scratch.iter_mut().zip(frames) {
            *into = match self.data.channels {
                InputChannels::Mono(channel) => [frame[channel]; Renderer::OUTPUT_CHANNELS],
                InputChannels::Stereo(left, right) => [frame[left], frame[right]],
            };
        }
    }

    /// Its handle asked it to stop.
    fn stopped(&self) -> bool {
        self.data.state.stop.load(Ordering::Relaxed)
    }

    /// The one way an input leaves.
    fn end(self, transfers: &mut transfer::RealtimeEnd<InputData>) {
        transfers.complete(self.data);
    }
}

#[derive(Debug)]
pub(crate) struct Inputs {
    /// Never past its capacity, so opening one never allocates.
    open: Vec<Input>,
    /// Samples per capture frame.
    capture_channels: usize,
    /// The only way inputs arrive or leave.
    transfers: transfer::RealtimeEnd<InputData>,
}

impl Inputs {
    pub(crate) fn new(
        places: usize,
        capture_channels: usize,
        transfers: transfer::RealtimeEnd<InputData>,
    ) -> Self {
        Self {
            open: Vec::with_capacity(places),
            capture_channels,
            transfers,
        }
    }

    /// New inputs open; stopped ones end.
    pub(crate) fn receive(&mut self) {
        while let Some(data) = self.transfers.try_recv() {
            let room = self.open.len() < self.open.capacity();
            debug_assert!(room, "one transfer per place");
            self.open.push(Input::open(data, self.capture_channels));
        }
        for input in self.open.extract_if(.., |input| input.stopped()) {
            input.end(&mut self.transfers);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Input> {
        self.open.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::engine::protocol::inputs::InputState;

    /// Received as the renderer receives it.
    fn input(channels: InputChannels, capture_channels: usize) -> Input {
        let (mut control, mut realtime) = transfer::channel(1);
        let state = Arc::new(InputState::default());
        let _transfer = control.try_start(InputData { channels, state }).unwrap();
        Input::open(realtime.try_recv().unwrap(), capture_channels)
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
