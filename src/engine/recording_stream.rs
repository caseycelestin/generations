use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use crate::engine::input::InputState;

#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    pub(crate) stop: AtomicBool,
    pub(crate) lost_frames: AtomicUsize,
    pub(crate) ended: AtomicBool,
}

#[derive(Debug)]
pub(crate) enum TapData {
    /// As heard, after clipping.
    MainOut,
    /// Whether it is monitored or not.
    Input(Arc<InputState>),
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Producer;

    use super::{RecordingState, TapData};
    use crate::engine::input::realtime::Input;
    use crate::engine::renderer::Frame;

    /// A point in the render loop a take may record from.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum TapPoint<'a> {
        MainOut,
        Input(&'a Input),
    }

    #[derive(Debug)]
    pub(crate) struct Recorder {
        frames: Producer<Frame>,
        take: Option<Take>,
    }

    #[derive(Debug)]
    struct Take {
        tap: TapData,
        state: Arc<RecordingState>,
    }

    impl Recorder {
        pub(crate) fn records_from(&self, point: TapPoint<'_>) -> bool {
            let Some(take) = &self.take else {
                return false;
            };
            match (point, &take.tap) {
                (TapPoint::MainOut, TapData::MainOut) => true,
                (TapPoint::Input(input), TapData::Input(state)) => {
                    Arc::ptr_eq(input.state(), state)
                }
                (TapPoint::MainOut, TapData::Input(_)) | (TapPoint::Input(_), TapData::MainOut) => {
                    false
                }
            }
        }

        /// A full ring takes the frames that fit and counts the rest lost,
        /// so a take loses its newest frames, never its oldest.
        pub(crate) fn write(&mut self, block: &[Frame]) {
            let Some(take) = &self.take else {
                return;
            };
            let fits = block.len().min(self.frames.slots());
            let (kept, lost) = block.split_at(fits);
            if let Ok(chunk) = self.frames.write_chunk_uninit(fits) {
                chunk.fill_from_iter(kept.iter().copied());
            }
            if !lost.is_empty() {
                take.state
                    .lost_frames
                    .fetch_add(lost.len(), Ordering::Relaxed);
            }
        }
    }
}
