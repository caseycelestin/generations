use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::sync::mpsc::{self, Receiver, SyncSender};

use rtrb::Consumer;
use thiserror::Error;

use crate::engine::renderer::Frame;

#[derive(Debug)]
pub(crate) struct RecordingState {
    /// Its index in both sides' lists, fixed at `create`.
    id: usize,
    /// The realtime side has pushed its last frame.
    idle: AtomicBool,
}

impl RecordingState {
    fn new(id: usize) -> Self {
        Self {
            id,
            idle: AtomicBool::new(true),
        }
    }
}

/// Nothing was armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ArmError {
    #[error("every recorder is in use")]
    Full,
}

#[derive(Debug)]
pub struct Recorder {
    state: Arc<RecordingState>,
}

impl Recorder {
    pub fn record(&self) {
        todo!()
    }
}

#[derive(Debug)]
pub struct Drain {
    consumer: Consumer<Frame>,
    state: Arc<RecordingState>,
    return_consumer: SyncSender<(usize, Consumer<Frame>)>,
}

#[derive(Debug)]
pub(crate) struct Recorders {
    states: Vec<Arc<RecordingState>>,
    return_consumer: SyncSender<(usize, Consumer<Frame>)>,
    returned_consumers: Receiver<(usize, Consumer<Frame>)>,
}

impl Recorders {
    pub(crate) fn arm(&mut self) -> Result<Recorder, ArmError> {
        let state = self.find_available().ok_or(ArmError::Full)?;
        Ok(Recorder {
            state: Arc::clone(state),
        })
    }

    /// Free once the realtime side is idle with it and no handle holds it,
    /// leaving the two sides' clones.
    fn find_available(&self) -> Option<&Arc<RecordingState>> {
        let state = self
            .states
            .iter()
            .find(|state| state.idle.load(Ordering::Acquire) && Arc::strong_count(state) == 2)?;
        // Pairs with the Release in a handle's drop, so its last writes are
        // seen before the state is reused.
        fence(Ordering::Acquire);
        Some(state)
    }
}

pub(crate) fn create(count: usize) -> (Recorders, realtime::Recorders) {
    let states: Vec<_> = (0..count)
        .map(|id| Arc::new(RecordingState::new(id)))
        .collect();
    let realtime = realtime::Recorders::new(states.clone());
    // Each recorder's consumer is out at most once, so a return never waits.
    let (return_consumer, returned_consumers) = mpsc::sync_channel(count);
    let recorders = Recorders {
        states,
        return_consumer,
        returned_consumers,
    };
    (recorders, realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;

    use super::RecordingState;

    #[derive(Debug)]
    pub(crate) struct Recorders {
        states: Vec<Arc<RecordingState>>,
    }

    impl Recorders {
        pub(crate) fn new(states: Vec<Arc<RecordingState>>) -> Self {
            Self { states }
        }
    }
}
