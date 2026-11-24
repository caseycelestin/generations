use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

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
}

impl InputState {
    fn new(id: usize) -> Self {
        Self {
            id: AtomicUsize::new(id),
            left: AtomicUsize::new(0),
            right: AtomicUsize::new(0),
            lifecycle: AtomicU8::new(Lifecycle::Idle as u8),
        }
    }
}

#[derive(Debug)]
#[must_use = "dropping it closes the input"]
pub struct Input {
    state: Arc<InputState>,
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

    use super::InputState;

    #[derive(Debug)]
    pub(crate) struct Inputs {
        states: Vec<Arc<InputState>>,
    }

    impl Inputs {
        pub(crate) fn new(states: Vec<Arc<InputState>>) -> Self {
            Self { states }
        }
    }
}
