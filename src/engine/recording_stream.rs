use std::sync::atomic::{AtomicBool, AtomicUsize};

#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    pub(crate) stop: AtomicBool,
    pub(crate) lost_frames: AtomicUsize,
    pub(crate) ended: AtomicBool,
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Producer;

    use super::RecordingState;
    use crate::engine::renderer::Frame;

    #[derive(Debug)]
    pub(crate) struct Recorder {
        frames: Producer<Frame>,
        state: Arc<RecordingState>,
    }

    impl Recorder {
        /// A full ring takes the frames that fit and counts the rest lost,
        /// so a take loses its newest frames, never its oldest.
        pub(crate) fn write(&mut self, block: &[Frame]) {
            let fits = block.len().min(self.frames.slots());
            let (kept, lost) = block.split_at(fits);
            if let Ok(chunk) = self.frames.write_chunk_uninit(fits) {
                chunk.fill_from_iter(kept.iter().copied());
            }
            if !lost.is_empty() {
                self.state
                    .lost_frames
                    .fetch_add(lost.len(), Ordering::Relaxed);
            }
        }
    }
}
