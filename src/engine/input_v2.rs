use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
}

impl InputState {
    fn new(id: usize) -> Self {
        Self {
            id: AtomicUsize::new(id),
            left: AtomicUsize::new(0),
            right: AtomicUsize::new(0),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Inputs {
    states: Vec<Arc<InputState>>,
}

impl Inputs {
    /// A state is free while only the two sides' clones hold it.
    pub(crate) fn open(&mut self, channels: InputChannels) -> Option<Arc<InputState>> {
        let free = self
            .states
            .iter()
            .find(|state| Arc::strong_count(state) == 2)?;
        // Relaxed: the store that opens the input must publish these.
        free.left.store(channels.left, Ordering::Relaxed);
        free.right.store(channels.right, Ordering::Relaxed);
        Some(Arc::clone(free))
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
