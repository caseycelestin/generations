//! An input: capture channels the renderer routes to stereo.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::engine::transfer::{self, ControlEnd};

/// Shared by both sides while the input is open.
#[derive(Debug, Default)]
pub(crate) struct InputState {
    pub(crate) stop: AtomicBool,
}

#[derive(Debug)]
pub(crate) struct InputData {
    pub(crate) channels: InputChannels,
    pub(crate) state: Arc<InputState>,
}

/// The capture device's channels an input reads, counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChannels {
    Mono(usize),
    /// Left, then right.
    Stereo(usize, usize),
}

/// Nothing was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InputError {
    /// Past the capture device's channels.
    #[error("no such channel")]
    NoSuchChannel,
    /// A stopped input gives its place back at the renderer's next period.
    #[error("every input is open")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<transfer::StartError<InputData>> for InputError {
    fn from(error: transfer::StartError<InputData>) -> Self {
        match error {
            transfer::StartError::Full(_) => Self::Full,
            transfer::StartError::Disconnected(_) => Self::RendererGone,
        }
    }
}

#[derive(Debug)]
pub struct Input {
    state: Arc<InputState>,
}

impl Input {
    pub(crate) fn open(
        transfers: &mut ControlEnd<InputData>,
        channels: InputChannels,
        capture_channels: usize,
    ) -> Result<Self, InputError> {
        let highest = match channels {
            InputChannels::Mono(channel) => channel,
            InputChannels::Stereo(left, right) => left.max(right),
        };
        if highest >= capture_channels {
            return Err(InputError::NoSuchChannel);
        }
        let state = Arc::new(InputState::default());
        let data = InputData {
            channels,
            state: Arc::clone(&state),
        };
        transfers.try_start(data)?;
        Ok(Self { state })
    }

    /// The same as dropping it.
    pub fn stop(self) {}
}

/// Unlike a voice, an input never ends by itself, so it ends with its
/// handle.
impl Drop for Input {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}

/// Inputs on the render side, held to the renderer's real-time rules: which
/// capture channels make each one's stereo frames, and the places they live.
/// They hold no audio.
pub(crate) mod realtime {
    use std::sync::atomic::Ordering;

    use super::{InputChannels, InputData};
    use crate::config::Pcm;
    use crate::engine::renderer::{Frame, Renderer};
    use crate::engine::transfer::{self, Completion};

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

        /// Its handle is gone.
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

        /// New inputs open, then stopped ones end, including any stopped before
        /// they arrived.
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
        use crate::engine::input::InputState;
        use crate::engine::transfer::Transfer;

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

        /// `places` inputs on a stereo device, and the control end that starts
        /// theirs, sized as `create` sizes it.
        fn inputs(places: usize) -> (transfer::ControlEnd<InputData>, Inputs) {
            let (control, realtime) = transfer::channel(places);
            (control, Inputs::new(places, 2, realtime))
        }

        /// Started as `Control::open_input` does.
        fn start(
            control: &mut transfer::ControlEnd<InputData>,
            channels: InputChannels,
        ) -> (Transfer<InputData>, Arc<InputState>) {
            let state = Arc::new(InputState::default());
            let data = InputData {
                channels,
                state: Arc::clone(&state),
            };
            (control.try_start(data).unwrap(), state)
        }

        fn open(inputs: &Inputs) -> Vec<InputChannels> {
            inputs.iter().map(|input| input.data.channels).collect()
        }

        fn ended(transfer: &mut Transfer<InputData>) -> bool {
            transfer.try_wait().is_ok()
        }

        #[test]
        fn received_inputs_open_in_the_order_sent() {
            let (mut control, mut inputs) = inputs(2);
            let (mut left, _) = start(&mut control, InputChannels::Mono(0));
            let (mut right, _) = start(&mut control, InputChannels::Mono(1));
            inputs.receive();
            assert_eq!(
                open(&inputs),
                [InputChannels::Mono(0), InputChannels::Mono(1)]
            );
            assert!(!ended(&mut left));
            assert!(!ended(&mut right));
        }

        #[test]
        fn a_stopped_input_ends_on_the_next_receive() {
            let (mut control, mut inputs) = inputs(1);
            let (mut input, state) = start(&mut control, InputChannels::Mono(0));
            inputs.receive();
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            assert!(open(&inputs).is_empty());
            assert!(ended(&mut input));
        }

        #[test]
        fn an_input_stopped_before_it_arrives_ends_on_arrival() {
            let (mut control, mut inputs) = inputs(1);
            let (mut input, state) = start(&mut control, InputChannels::Mono(0));
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            assert!(open(&inputs).is_empty());
            assert!(ended(&mut input));
        }

        #[test]
        fn an_ended_input_gives_its_place_back() {
            let (mut control, mut inputs) = inputs(1);
            let (_input, state) = start(&mut control, InputChannels::Mono(0));
            inputs.receive();
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            let state = Arc::new(InputState::default());
            let data = InputData {
                channels: InputChannels::Mono(1),
                state,
            };
            assert!(control.try_start(data).is_ok());
        }
    }
}
