use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

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

/// One voice of a loaded audio, driven by atomics alone.
#[derive(Debug, Default)]
pub(crate) struct VoiceSlot {
    /// The number of a voice waiting to start, or 0. The renderer takes it,
    /// leaving 0, so a request is seen once.
    pub(crate) pending: AtomicU32,
}

impl VoiceSlot {
    /// The renderer's: the number of the voice waiting to start, if any.
    pub(crate) fn take_pending(&self) -> Option<u32> {
        // Read first, so a slot with nothing waiting is never written.
        if self.pending.load(Ordering::Relaxed) == 0 {
            return None;
        }
        match self.pending.swap(0, Ordering::Acquire) {
            0 => None,
            number => Some(number),
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

pub struct Audio<const VOICES: usize> {
    state: Arc<AudioState<VOICES>>,
}

impl<const VOICES: usize> Audio<VOICES> {
    pub(crate) fn load(state: Arc<AudioState<VOICES>>) -> Self {
        Self { state }
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

    use super::{AudioBuffer, AudioState};
    use crate::engine::math::add;
    use crate::engine::renderer::{Frame, OUTPUT_CHANNELS};
    use crate::layout::ChannelLayout;

    /// One playing voice of a loaded audio; only the renderer holds it.
    #[derive(Debug)]
    pub(crate) struct Voice {
        /// The next frame to play, counted from the start of the buffer.
        playhead: usize,
    }

    impl Voice {
        /// Silence once the buffer runs out. Mono goes to both sides at full
        /// level.
        fn render(&mut self, buffer: &AudioBuffer, scratch: &mut [Frame]) {
            let channels = buffer.layout.count();
            let frames = buffer.samples.len() / channels;
            let end = (self.playhead + scratch.len()).min(frames);
            let samples = &buffer.samples[self.playhead * channels..end * channels];
            let (sound, silence) = scratch.split_at_mut(end - self.playhead);
            match buffer.layout {
                ChannelLayout::Mono => {
                    for (frame, &sample) in sound.iter_mut().zip(samples) {
                        *frame = [sample, sample];
                    }
                }
                ChannelLayout::Stereo => {
                    let (pairs, _) = samples.as_chunks::<2>();
                    sound.copy_from_slice(pairs);
                }
            }
            silence.fill([0.0; OUTPUT_CHANNELS]);
            self.playhead = end;
        }

        /// Its last frame has been rendered.
        fn finished(&self, buffer: &AudioBuffer) -> bool {
            self.playhead == buffer.samples.len() / buffer.layout.count()
        }
    }

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
                if slot.take_pending().is_some() {
                    *voice = Some(Voice { playhead: 0 });
                }
                if let Some(playing) = voice {
                    playing.render(buffer, scratch);
                    add(out, scratch);
                    if playing.finished(buffer) {
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
