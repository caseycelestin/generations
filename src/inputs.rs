use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Lifecycle {
    /// Free for `open`.
    Idle,
    /// Rendered each block from its channels.
    Open,
    /// Its handle is gone; only the renderer moves it on, to `Idle`, once
    /// it no longer reads its channels.
    CloseRequested,
}

impl Lifecycle {
    fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Idle as u8 => Self::Idle,
            v if v == Self::Open as u8 => Self::Open,
            v if v == Self::CloseRequested as u8 => Self::CloseRequested,
            _ => unreachable!("only a Lifecycle is stored"),
        }
    }
}

/// The capture device's channels an input reads, counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputChannels {
    pub left: usize,
    pub right: usize,
}

#[derive(Debug)]
pub(crate) struct InputState {
    /// Capture channels; a mono source names the same one twice.
    left: AtomicUsize,
    right: AtomicUsize,
    /// A `Lifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
    /// Off when the input opens, so an open mic is not heard until asked for.
    monitoring: AtomicBool,
}

impl InputState {
    fn new() -> Self {
        Self {
            left: AtomicUsize::new(0),
            right: AtomicUsize::new(0),
            lifecycle: AtomicU8::new(Lifecycle::Idle as u8),
            monitoring: AtomicBool::new(false),
        }
    }
}

#[derive(Debug)]
#[must_use = "dropping it closes the input"]
pub struct Input {
    state: Arc<InputState>,
}

impl Input {
    /// Takes effect from the renderer's next block.
    pub fn set_monitoring(&mut self, on: bool) {
        self.state.monitoring.store(on, Ordering::Relaxed);
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        // A plain store: only the handle moves a state out of `Open`.
        self.state
            .lifecycle
            .store(Lifecycle::CloseRequested as u8, Ordering::Release);
    }
}

/// Nothing was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InputError {
    /// Past the capture device's channels.
    #[error("no such channel")]
    NoSuchChannel,
    /// A closed input gives its place back at the renderer's next block.
    #[error("every input is open")]
    Full,
}

#[derive(Debug)]
pub struct Inputs {
    states: Box<[Arc<InputState>]>,
    /// The capture device's, so `open` can refuse channels past it.
    capture_channels: usize,
}

impl Inputs {
    /// Inputs may read the same channels; each gets its own copy.
    ///
    /// ```no_run
    /// use generations::InputChannels;
    ///
    /// // A mono device, with one input.
    /// let (mut control, mut renderer) = generations::create(1, 1, 0, 8, 0, 0);
    /// // Mono: the same channel on both sides.
    /// let mut input = control.inputs().open(InputChannels { left: 0, right: 0 })?;
    /// // Heard through the main out until closed.
    /// input.set_monitoring(true);
    ///
    /// // A duplex callback would own the renderer: a period of capture in,
    /// // the same frames out.
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    ///
    /// // Silent, and its place free, from the renderer's next block.
    /// drop(input);
    /// renderer.render(&[0.0; 512], &mut [0.0; 512 * 2]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A state is free while only the two sides' clones hold it.
    pub fn open(&mut self, channels: InputChannels) -> Result<Input, InputError> {
        if channels.left.max(channels.right) >= self.capture_channels {
            return Err(InputError::NoSuchChannel);
        }
        let free = self.states.iter().find(|state| {
            // Acquire: pairs with the store that closes the input.
            state.lifecycle.load(Ordering::Acquire) == Lifecycle::Idle as u8
                && Arc::strong_count(state) == 2
        });
        let free = free.ok_or(InputError::Full)?;
        // Relaxed: the store that opens the input must publish these.
        free.left.store(channels.left, Ordering::Relaxed);
        free.right.store(channels.right, Ordering::Relaxed);
        free.monitoring.store(false, Ordering::Relaxed);
        free.lifecycle
            .store(Lifecycle::Open as u8, Ordering::Release);
        Ok(Input {
            state: Arc::clone(free),
        })
    }
}

pub(crate) fn create(count: usize, capture_channels: usize) -> (Inputs, realtime::Inputs) {
    let states: Box<[Arc<InputState>]> = (0..count).map(|_| Arc::new(InputState::new())).collect();
    let realtime = realtime::Inputs::new(states.clone(), capture_channels);
    let inputs = Inputs {
        states,
        capture_channels,
    };
    (inputs, realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::{InputState, Lifecycle};
    use crate::format::Frame;

    #[derive(Debug)]
    pub(crate) struct Input {
        state: Arc<InputState>,
        /// Samples per capture frame.
        capture_channels: usize,
    }

    impl Input {
        /// True if the block is rendered.
        fn advance_lifecycle(&self) -> bool {
            let lifecycle = &self.state.lifecycle;
            // Acquire takes the channels `open` wrote.
            match Lifecycle::from_u8(lifecycle.load(Ordering::Acquire)) {
                Lifecycle::Open => true,
                // A plain store: only the renderer moves a state out of
                // `CloseRequested`. Release: the channels are no longer
                // read, so `open` may rewrite them.
                Lifecycle::CloseRequested => {
                    lifecycle.store(Lifecycle::Idle as u8, Ordering::Release);
                    false
                }
                Lifecycle::Idle => false,
            }
        }

        /// Called every block, so opens and closes land on block boundaries.
        /// Silence unless open, so a stale `scratch` is never heard.
        pub(crate) fn render(&self, capture: &[f32], scratch: &mut [Frame]) {
            if !self.advance_lifecycle() {
                scratch.fill([0.0; 2]);
                return;
            }
            // Relaxed: `advance_lifecycle`'s Acquire already took them.
            let left = self.state.left.load(Ordering::Relaxed);
            let right = self.state.right.load(Ordering::Relaxed);
            let frames = capture.chunks_exact(self.capture_channels);
            for (into, frame) in scratch.iter_mut().zip(frames) {
                *into = [frame[left], frame[right]];
            }
        }

        pub(crate) fn monitoring(&self) -> bool {
            self.state.monitoring.load(Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    pub(crate) struct Inputs {
        inputs: Box<[Input]>,
    }

    impl Inputs {
        pub(crate) fn new(states: Box<[Arc<InputState>]>, capture_channels: usize) -> Self {
            let inputs = states
                .into_iter()
                .map(|state| Input {
                    state,
                    capture_channels,
                })
                .collect();
            Self { inputs }
        }

        pub(crate) fn iter(&self) -> impl Iterator<Item = &Input> {
            self.inputs.iter()
        }
    }
}
