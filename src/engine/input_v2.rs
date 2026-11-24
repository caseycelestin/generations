use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

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
    /// Only ever given what was stored from a `Lifecycle`.
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
    id: AtomicUsize,
    /// Capture channels; a mono source names the same one twice.
    left: AtomicUsize,
    right: AtomicUsize,
    /// A `Lifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
    /// Off when the input opens, so an open mic is not heard until asked for.
    monitoring: AtomicBool,
}

impl InputState {
    fn new(id: usize) -> Self {
        Self {
            id: AtomicUsize::new(id),
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

#[derive(Debug)]
pub(crate) struct Inputs {
    states: Vec<Arc<InputState>>,
}

impl Inputs {
    /// A state is free while only the two sides' clones hold it.
    pub(crate) fn open(&mut self, channels: InputChannels) -> Option<Input> {
        let free = self.states.iter().find(|state| {
            // Acquire: pairs with the store that closes the input.
            state.lifecycle.load(Ordering::Acquire) == Lifecycle::Idle as u8
                && Arc::strong_count(state) == 2
        })?;
        // Relaxed: the store that opens the input must publish these.
        free.left.store(channels.left, Ordering::Relaxed);
        free.right.store(channels.right, Ordering::Relaxed);
        free.monitoring.store(false, Ordering::Relaxed);
        free.lifecycle
            .store(Lifecycle::Open as u8, Ordering::Release);
        Some(Input {
            state: Arc::clone(free),
        })
    }
}

pub(crate) fn create(count: usize) -> (Inputs, realtime::Inputs) {
    let states: Vec<Arc<InputState>> = (0..count).map(|id| Arc::new(InputState::new(id))).collect();
    let realtime = realtime::Inputs::new(states.clone());
    (Inputs { states }, realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;

    use std::sync::atomic::Ordering;

    use super::{InputState, Lifecycle};

    #[derive(Debug)]
    pub(crate) struct Input {
        state: Arc<InputState>,
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

        pub(crate) fn monitoring(&self) -> bool {
            self.state.monitoring.load(Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    pub(crate) struct Inputs {
        inputs: Vec<Input>,
    }

    impl Inputs {
        pub(crate) fn new(states: Vec<Arc<InputState>>) -> Self {
            let inputs = states.into_iter().map(|state| Input { state }).collect();
            Self { inputs }
        }
    }
}
