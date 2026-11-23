use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering, fence};
use std::sync::mpsc::{self, Receiver, SyncSender};

use rtrb::{Consumer, RingBuffer};
use thiserror::Error;

use crate::engine::renderer::Frame;

/// Where a recorder is in its life. Each side moves it only by
/// `compare_exchange` from the state it expects, so neither overwrites the
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Lifecycle {
    /// No `Recorder` holds it; only `arm` moves it on.
    Idle,
    /// Held by a `Recorder`, not recording.
    Armed,
    /// `record` asked; the realtime side has not started.
    StartRequested,
    /// The realtime side is writing frames.
    Recording,
    /// `stop` asked; the realtime side ends at `Armed` after its last frame.
    StopRequested,
    /// Its `Recorder` was dropped while recording; the realtime side ends at
    /// `Idle` after its last frame.
    Releasing,
}

impl Lifecycle {
    /// Only ever given what was stored from a `Lifecycle`.
    fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Idle as u8 => Self::Idle,
            v if v == Self::Armed as u8 => Self::Armed,
            v if v == Self::StartRequested as u8 => Self::StartRequested,
            v if v == Self::Recording as u8 => Self::Recording,
            v if v == Self::StopRequested as u8 => Self::StopRequested,
            v if v == Self::Releasing as u8 => Self::Releasing,
            _ => unreachable!("only a Lifecycle is stored"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RecordingState {
    /// Its index in both sides' lists, fixed at `create`.
    id: usize,
    /// A `Lifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
}

impl RecordingState {
    fn new(id: usize) -> Self {
        Self {
            id,
            lifecycle: AtomicU8::new(Lifecycle::Idle as u8),
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
    /// Does nothing unless armed and not already recording.
    pub fn record(&self) {
        // Release hands what `arm` prepared to the realtime side.
        let _ = self.state.lifecycle.compare_exchange(
            Lifecycle::Armed as u8,
            Lifecycle::StartRequested as u8,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    /// Does nothing unless recording.
    pub fn stop(&self) {
        // Relaxed: only a request; the realtime side publishes the end.
        let _ = self.state.lifecycle.compare_exchange(
            Lifecycle::Recording as u8,
            Lifecycle::StopRequested as u8,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

/// Frees the recorder, or, mid-recording, leaves the realtime side to free
/// it after its last frame.
impl Drop for Recorder {
    fn drop(&mut self) {
        // Retried if the realtime side moves it meanwhile, so a start it
        // takes ends here as `Releasing`, never `Idle`.
        let lifecycle = &self.state.lifecycle;
        let _ = lifecycle.try_update(Ordering::Release, Ordering::Relaxed, |current| {
            let next = match Lifecycle::from_u8(current) {
                Lifecycle::Armed | Lifecycle::StartRequested => Lifecycle::Idle,
                Lifecycle::Recording | Lifecycle::StopRequested => Lifecycle::Releasing,
                Lifecycle::Idle | Lifecycle::Releasing => return None,
            };
            Some(next as u8)
        });
    }
}

#[derive(Debug)]
pub struct Drain {
    /// `None` only once dropped, so `drop` can send it home.
    consumer: Option<Consumer<Frame>>,
    state: Arc<RecordingState>,
    return_consumer: SyncSender<(usize, Consumer<Frame>)>,
}

/// Sends the consumer home to its slot.
impl Drop for Drain {
    fn drop(&mut self) {
        // Sent here, before `state` drops with the fields after this, so a
        // slot `arm` finds free already has its consumer on the way.
        if let Some(consumer) = self.consumer.take() {
            let _ = self.return_consumer.send((self.state.id, consumer));
        }
    }
}

#[derive(Debug)]
struct Slot {
    state: Arc<RecordingState>,
    /// `None` while lent to a `Drain`.
    consumer: Option<Consumer<Frame>>,
}

#[derive(Debug)]
pub(crate) struct Recorders {
    /// By id.
    slots: Vec<Slot>,
    return_consumer: SyncSender<(usize, Consumer<Frame>)>,
    returned_consumers: Receiver<(usize, Consumer<Frame>)>,
}

impl Recorders {
    pub(crate) fn arm(&mut self) -> Result<(Recorder, Drain), ArmError> {
        let id = self.find_available().ok_or(ArmError::Full)?.state.id;
        // After the search: a `Drain` sends its consumer before letting go
        // of the state, so the one for a slot found free is already sent.
        self.collect();
        let slot = &mut self.slots[id];
        // Relaxed: nothing is handed to the realtime side until `record`.
        slot.state
            .lifecycle
            .compare_exchange(
                Lifecycle::Idle as u8,
                Lifecycle::Armed as u8,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .map_err(|_| ArmError::Full)?;
        let recorder = Recorder {
            state: Arc::clone(&slot.state),
        };
        let drain = Drain {
            consumer: Some(
                slot.consumer
                    .take()
                    .expect("a free slot's consumer is home"),
            ),
            state: Arc::clone(&slot.state),
            return_consumer: self.return_consumer.clone(),
        };
        Ok((recorder, drain))
    }

    /// Puts each consumer a `Drain` sent home back in its slot.
    fn collect(&mut self) {
        for (id, consumer) in self.returned_consumers.try_iter() {
            self.slots[id].consumer = Some(consumer);
        }
    }

    /// Free once `Idle` and no handle holds it, leaving the two sides' clones;
    /// a `Drain` can outlive its `Recorder`, so `Idle` alone is not enough.
    fn find_available(&mut self) -> Option<&mut Slot> {
        let slot = self.slots.iter_mut().find(|slot| {
            slot.state.lifecycle.load(Ordering::Acquire) == Lifecycle::Idle as u8
                && Arc::strong_count(&slot.state) == 2
        })?;
        // Pairs with the Release in a handle's drop, so its last writes are
        // seen before the state is reused.
        fence(Ordering::Acquire);
        Some(slot)
    }
}

pub(crate) fn create(count: usize, ring_frames: usize) -> (Recorders, realtime::Recorders) {
    let mut slots = Vec::with_capacity(count);
    let mut realtime_recorders = Vec::with_capacity(count);
    for id in 0..count {
        let state = Arc::new(RecordingState::new(id));
        let (producer, consumer) = RingBuffer::new(ring_frames);
        realtime_recorders.push(realtime::Recorder::new(Arc::clone(&state), producer));
        slots.push(Slot {
            state,
            consumer: Some(consumer),
        });
    }
    let realtime = realtime::Recorders::new(realtime_recorders);
    // Each recorder's consumer is out at most once, so a return never waits.
    let (return_consumer, returned_consumers) = mpsc::sync_channel(count);
    let recorders = Recorders {
        slots,
        return_consumer,
        returned_consumers,
    };
    (recorders, realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;

    use rtrb::Producer;

    use super::RecordingState;
    use crate::engine::renderer::Frame;

    #[derive(Debug)]
    pub(crate) struct Recorder {
        state: Arc<RecordingState>,
        producer: Producer<Frame>,
    }

    impl Recorder {
        pub(crate) fn new(state: Arc<RecordingState>, producer: Producer<Frame>) -> Self {
            Self { state, producer }
        }
    }

    #[derive(Debug)]
    pub(crate) struct Recorders {
        /// By id, for the whole run.
        recorders: Vec<Recorder>,
    }

    impl Recorders {
        pub(crate) fn new(recorders: Vec<Recorder>) -> Self {
            Self { recorders }
        }
    }
}
