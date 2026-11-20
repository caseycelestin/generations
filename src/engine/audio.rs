use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::engine::voice::{self, VoiceSlot};
use crate::layout::ChannelLayout;

/// `VOICES` is how many voices each loaded audio has; fixed in the type, so
/// replacing a place's state never allocates.
#[derive(Debug)]
pub(crate) struct AudioState<const VOICES: usize> {
    /// `None` while its place is free.
    pub(crate) buffer: Option<AudioBuffer>,
    pub(crate) unload: AtomicBool,
    pub(crate) voices: [VoiceSlot; VOICES],
}

/// By hand: std's `Default` for arrays stops at 32.
impl<const VOICES: usize> Default for AudioState<VOICES> {
    fn default() -> Self {
        Self {
            buffer: None,
            unload: AtomicBool::default(),
            voices: std::array::from_fn(|_| VoiceSlot::default()),
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

#[must_use = "dropping it unloads the audio"]
pub struct Audio<const VOICES: usize> {
    state: Arc<AudioState<VOICES>>,
    /// Voices asked for so far; the next one's number is this plus one, so
    /// none is 0.
    played: u32,
    /// The number last sent to each slot, or 0; a slot is free once the
    /// renderer has finished that voice.
    sent: [u32; VOICES],
}

impl<const VOICES: usize> Audio<VOICES> {
    pub(crate) fn load(state: Arc<AudioState<VOICES>>) -> Self {
        Self {
            state,
            played: 0,
            sent: [0; VOICES],
        }
    }

    /// Heard from the renderer's next block, from the top, in a free slot;
    /// with none free, it cuts the oldest voice, since a press must sound.
    pub fn play(&mut self) -> voice::Voice<VOICES> {
        let free = (0..VOICES).find(|&slot| self.state.voices[slot].finished(self.sent[slot]));
        // Numbers only grow, so the smallest sent is the oldest voice.
        let oldest = || (0..VOICES).min_by_key(|&slot| self.sent[slot]);
        let slot = free
            .or_else(oldest)
            .expect("an audio has at least one voice");
        self.played += 1;
        self.sent[slot] = self.played;
        self.state.voices[slot].send_voice(self.played);
        voice::Voice::play(Arc::clone(&self.state), slot, self.played)
    }

    /// The same as dropping it.
    pub fn unload(self) {}
}

/// Loaded audio stays loaded while its handle is held.
impl<const VOICES: usize> Drop for Audio<VOICES> {
    fn drop(&mut self) {
        self.state.unload.store(true, Ordering::Relaxed);
    }
}

/// `places` is how many can be loaded at once.
pub(crate) fn channel<const VOICES: usize>(
    places: usize,
) -> (
    Producer<Arc<AudioState<VOICES>>>,
    realtime::LoadedAudio<VOICES>,
) {
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
    use crate::engine::voice::realtime::Voice;

    #[derive(Debug)]
    pub(crate) struct Audio<const VOICES: usize> {
        state: Arc<AudioState<VOICES>>,
        /// One per voice slot, `None` while it is silent.
        voices: [Option<Voice>; VOICES],
    }

    impl<const VOICES: usize> Audio<VOICES> {
        fn load(state: Arc<AudioState<VOICES>>) -> Self {
            Self {
                state,
                voices: [const { None }; VOICES],
            }
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
            for (voice, slot) in self.voices.iter_mut().zip(&self.state.voices) {
                if let Some(number) = slot.take_incoming_voice() {
                    *voice = Some(Voice::start(number));
                }
                if let Some(playing) = voice
                    && slot.stopped(playing.number())
                {
                    slot.finish(playing.number());
                    *voice = None;
                }
                if let Some(playing) = voice {
                    playing.render(buffer, scratch);
                    add(out, scratch);
                    if playing.finished(buffer) {
                        slot.finish(playing.number());
                        *voice = None;
                    }
                }
            }
        }
    }

    #[derive(Debug)]
    pub(crate) struct LoadedAudio<const VOICES: usize> {
        /// Never past its capacity, so loading one never allocates.
        audio: Vec<Audio<VOICES>>,
        /// The only way audio arrives.
        loaded: Consumer<Arc<AudioState<VOICES>>>,
    }

    impl<const VOICES: usize> LoadedAudio<VOICES> {
        pub(crate) fn new(places: usize, loaded: Consumer<Arc<AudioState<VOICES>>>) -> Self {
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

        pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Audio<VOICES>> {
            self.audio.iter_mut()
        }
    }
}
