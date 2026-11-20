use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::layout::ChannelLayout;

#[derive(Debug, Default)]
pub(crate) struct AudioState {
    /// `None` while its place is free.
    pub(crate) buffer: Option<AudioBuffer>,
    pub(crate) unload: AtomicBool,
}

/// Lives in the state, so the control side's pool holds it until the
/// place is reused and the renderer never frees it.
#[derive(Debug)]
pub(crate) struct AudioBuffer {
    pub(crate) samples: Vec<f32>,
    pub(crate) layout: ChannelLayout,
}

/// Nothing was loaded; the samples come back in it.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("the samples end part-way through a frame")]
    PartFrame(Vec<f32>),
    #[error("every place for loaded audio is held")]
    Full(Vec<f32>),
}

impl LoadError {
    pub fn into_samples(self) -> Vec<f32> {
        match self {
            Self::PartFrame(samples) | Self::Full(samples) => samples,
        }
    }
}

pub struct Audio {
    state: Arc<AudioState>,
}

impl Audio {
    pub(crate) fn load(state: Arc<AudioState>) -> Self {
        Self { state }
    }

    /// The same as dropping it.
    pub fn unload(self) {}
}

/// Loaded audio stays loaded while its handle is held.
impl Drop for Audio {
    fn drop(&mut self) {
        self.state.unload.store(true, Ordering::Relaxed);
    }
}

/// `places` is how many can be loaded at once.
pub(crate) fn channel(places: usize) -> (Producer<Arc<AudioState>>, realtime::LoadedAudio) {
    let (producer, consumer) = RingBuffer::new(places);
    (producer, realtime::LoadedAudio::new(places, consumer))
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Consumer;

    use super::AudioState;

    #[derive(Debug)]
    pub(crate) struct Audio {
        state: Arc<AudioState>,
    }

    impl Audio {
        fn load(state: Arc<AudioState>) -> Self {
            Self { state }
        }

        /// Its handle unloaded it or is gone.
        fn unloaded(&self) -> bool {
            self.state.unload.load(Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    pub(crate) struct LoadedAudio {
        /// Never past its capacity, so loading one never allocates.
        audio: Vec<Audio>,
        /// The only way audio arrives.
        loaded: Consumer<Arc<AudioState>>,
    }

    impl LoadedAudio {
        pub(crate) fn new(places: usize, loaded: Consumer<Arc<AudioState>>) -> Self {
            Self {
                audio: Vec::with_capacity(places),
                loaded,
            }
        }

        /// New audio arrives, then unloaded audio leaves, including any
        /// unloaded before it arrived.
        pub(crate) fn receive(&mut self) {
            while let Ok(state) = self.loaded.pop() {
                let room = self.audio.len() < self.audio.capacity();
                debug_assert!(room, "one message per place");
                self.audio.push(Audio::load(state));
            }
            // Never the last clone of a state: the control side's pool keeps
            // one, so no samples are freed here.
            self.audio.retain(|audio| !audio.unloaded());
        }

        pub(crate) fn iter(&self) -> impl Iterator<Item = &Audio> {
            self.audio.iter()
        }
    }
}
