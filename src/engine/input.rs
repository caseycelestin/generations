//! An input: capture channels the renderer routes to stereo.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

#[derive(Debug, Default)]
pub(crate) struct InputState {
    stop: AtomicBool,
    monitoring: AtomicBool,
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

#[derive(Debug)]
#[must_use = "dropping it stops the input"]
pub struct Input {
    state: Arc<InputState>,
}

impl Input {
    pub(crate) fn open(state: Arc<InputState>) -> Self {
        Self { state }
    }

    /// Off when the input opens, so an open mic is not heard until asked
    /// for. Takes effect from the renderer's next block.
    pub fn set_monitoring(&mut self, on: bool) {
        self.state.monitoring.store(on, Ordering::Relaxed);
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

/// `inputs` is how many can be open at once; `capture_channels` is the width
/// of each period's capture.
pub(crate) fn channel(
    inputs: usize,
    capture_channels: usize,
) -> (Producer<InputData>, realtime::Inputs) {
    let (producer, consumer) = RingBuffer::new(inputs);
    (
        producer,
        realtime::Inputs::new(inputs, capture_channels, consumer),
    )
}

/// Which capture channels make each input's stereo frames, and the places
/// they live. They hold no audio.
pub(crate) mod realtime {
    use std::sync::atomic::Ordering;

    use rtrb::Consumer;

    use super::{InputChannels, InputData};
    use crate::config::Pcm;
    use crate::engine::renderer::{Frame, OUTPUT_CHANNELS};

    #[derive(Debug)]
    pub(crate) struct Input {
        data: InputData,
        /// Samples per capture frame.
        capture_channels: usize,
    }

    impl Input {
        fn open(data: InputData, capture_channels: usize) -> Self {
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
                    InputChannels::Mono(channel) => [frame[channel]; OUTPUT_CHANNELS],
                    InputChannels::Stereo(left, right) => [frame[left], frame[right]],
                };
            }
        }

        pub(crate) fn monitoring(&self) -> bool {
            self.data.state.monitoring.load(Ordering::Relaxed)
        }

        /// Its handle is gone.
        fn stopped(&self) -> bool {
            self.data.state.stop.load(Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    pub(crate) struct Inputs {
        /// Never past its capacity, so opening one never allocates.
        open: Vec<Input>,
        capture_channels: usize,
        /// The only way inputs arrive.
        opened: Consumer<InputData>,
    }

    impl Inputs {
        pub(crate) fn new(
            places: usize,
            capture_channels: usize,
            opened: Consumer<InputData>,
        ) -> Self {
            Self {
                open: Vec::with_capacity(places),
                capture_channels,
                opened,
            }
        }

        /// New inputs open, then stopped ones end, including any stopped before
        /// they arrived.
        pub(crate) fn receive(&mut self) {
            while let Ok(data) = self.opened.pop() {
                let room = self.open.len() < self.open.capacity();
                debug_assert!(room, "one message per place");
                self.open.push(Input::open(data, self.capture_channels));
            }
            // Never the last clone of a state: the control side's pool keeps
            // one, so nothing is freed here.
            self.open.retain(|input| !input.stopped());
        }

        pub(crate) fn iter(&self) -> impl Iterator<Item = &Input> {
            self.open.iter()
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;

        use rtrb::Producer;

        use super::*;
        use crate::engine::input::{self, InputState};

        fn input(channels: InputChannels, capture_channels: usize) -> Input {
            let state = Arc::new(InputState::default());
            Input::open(InputData { channels, state }, capture_channels)
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

        /// `places` inputs on a stereo device, and the ring that opens them,
        /// sized as `create` sizes it.
        fn inputs(places: usize) -> (Producer<InputData>, Inputs) {
            input::channel(places, 2)
        }

        /// Sent as `Control::open_input` sends it; the state returned stands in
        /// for the pool's clone.
        fn start(control: &mut Producer<InputData>, channels: InputChannels) -> Arc<InputState> {
            let state = Arc::new(InputState::default());
            let data = InputData {
                channels,
                state: Arc::clone(&state),
            };
            control.push(data).unwrap();
            state
        }

        fn open(inputs: &Inputs) -> Vec<InputChannels> {
            inputs.iter().map(|input| input.data.channels).collect()
        }

        /// The renderer has let go of its clone.
        fn ended(state: &Arc<InputState>) -> bool {
            Arc::strong_count(state) == 1
        }

        #[test]
        fn received_inputs_open_in_the_order_sent() {
            let (mut control, mut inputs) = inputs(2);
            let left = start(&mut control, InputChannels::Mono(0));
            let right = start(&mut control, InputChannels::Mono(1));
            inputs.receive();
            assert_eq!(
                open(&inputs),
                [InputChannels::Mono(0), InputChannels::Mono(1)]
            );
            assert!(!ended(&left));
            assert!(!ended(&right));
        }

        #[test]
        fn a_stopped_input_ends_on_the_next_receive() {
            let (mut control, mut inputs) = inputs(1);
            let state = start(&mut control, InputChannels::Mono(0));
            inputs.receive();
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            assert!(open(&inputs).is_empty());
            assert!(ended(&state));
        }

        #[test]
        fn an_input_stopped_before_it_arrives_ends_on_arrival() {
            let (mut control, mut inputs) = inputs(1);
            let state = start(&mut control, InputChannels::Mono(0));
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            assert!(open(&inputs).is_empty());
            assert!(ended(&state));
        }

        #[test]
        fn an_ended_input_gives_its_place_back() {
            let (mut control, mut inputs) = inputs(1);
            let state = start(&mut control, InputChannels::Mono(0));
            inputs.receive();
            state.stop.store(true, Ordering::Relaxed);
            inputs.receive();
            start(&mut control, InputChannels::Mono(1));
            inputs.receive();
            assert_eq!(open(&inputs), [InputChannels::Mono(1)]);
        }
    }
}
