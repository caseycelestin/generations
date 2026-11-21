use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::engine::voice::{self, VoiceSlot};
use crate::layout::ChannelLayout;

#[derive(Debug)]
pub(crate) struct AudioState {
    /// `None` while its place is free.
    pub(crate) buffer: Option<AudioBuffer>,
    pub(crate) unload: AtomicBool,
    /// Made once, at `create`; reset rather than replaced when the place is
    /// reused, so a load never allocates.
    pub(crate) voices: Box<[VoiceSlot]>,
}

impl AudioState {
    /// An empty place with room for `voices` voices. Allocates, so only at
    /// `create`.
    pub(crate) fn new(voices: usize) -> Self {
        Self {
            buffer: None,
            unload: AtomicBool::new(false),
            voices: (0..voices).map(|_| VoiceSlot::default()).collect(),
        }
    }

    /// Ready for a new load, in place: every field a reused place carries
    /// over is reset here, and the slots made at `create` are kept.
    pub(crate) fn refill(&mut self, buffer: AudioBuffer) {
        self.buffer = Some(buffer);
        *self.unload.get_mut() = false;
        for slot in &mut self.voices {
            slot.reset();
        }
    }
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
    #[error("the renderer is gone")]
    RendererGone(Vec<f32>),
}

impl LoadError {
    pub fn into_samples(self) -> Vec<f32> {
        match self {
            Self::PartFrame(samples) | Self::Full(samples) | Self::RendererGone(samples) => samples,
        }
    }
}

#[derive(Debug)]
#[must_use = "dropping it unloads the audio"]
pub struct Audio {
    state: Arc<AudioState>,
    /// Voices asked for so far; the next one's number is this plus one, so
    /// none is 0.
    played: u32,
}

impl Audio {
    pub(crate) fn load(state: Arc<AudioState>) -> Self {
        Self { state, played: 0 }
    }

    /// Heard from the renderer's next block, from the top, in a free slot;
    /// with none free, it cuts the oldest voice, since a press must sound.
    pub fn play(&mut self) -> voice::Voice {
        let slots = &self.state.voices;
        let free = slots.iter().position(VoiceSlot::is_free);
        let oldest = || (0..slots.len()).min_by_key(|&slot| slots[slot].last_sent());
        let slot = free
            .or_else(oldest)
            .expect("an audio has at least one voice");
        self.played += 1;
        slots[slot].send_voice(self.played);
        voice::Voice::play(Arc::clone(&self.state), slot, self.played)
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
    use crate::engine::math::add;
    use crate::engine::renderer::Frame;
    use crate::engine::voice::realtime as voice;

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

        /// Adds each of its playing voices into `out`, one at a time through
        /// `scratch`.
        pub(crate) fn render(&mut self, out: &mut [Frame], scratch: &mut [Frame]) {
            // Always there once loaded; silence rather than a panic here.
            let Some(buffer) = &self.state.buffer else {
                return;
            };
            for slot in &self.state.voices {
                if let Some(number) = slot.take_incoming_voice() {
                    slot.start(number);
                }
                if let Some(number) = slot.playing()
                    && slot.stopped(number)
                {
                    slot.finish(number);
                }
                if let Some(number) = slot.playing() {
                    voice::render(slot, buffer, scratch);
                    add(out, scratch);
                    if voice::finished(slot, buffer) {
                        slot.finish(number);
                    }
                }
            }
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

        pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Audio> {
            self.audio.iter_mut()
        }
    }
}
