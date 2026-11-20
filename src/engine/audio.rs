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
    incoming_voice: AtomicU32,
    /// Raised to a stopped voice's number, never lowered, so an older
    /// voice's handle cannot stop a newer one; the playing voice stops once
    /// this reaches its number.
    stop: AtomicU32,
    /// The number of the last voice the renderer finished here, played to
    /// its end or stopped; the slot is free once this is the last one sent.
    done: AtomicU32,
}

impl VoiceSlot {
    /// The control side's: asks for voice `number` to start at the
    /// renderer's next block, replacing any request not yet taken.
    pub(crate) fn send_voice(&self, number: u32) {
        self.incoming_voice.store(number, Ordering::Release);
    }

    /// The control side's: asks for voice `number` to stop at the renderer's
    /// next block, if it is still the one in this slot.
    pub(crate) fn request_stop(&self, number: u32) {
        self.stop.fetch_max(number, Ordering::Release);
    }

    /// The renderer's: voice `number` has been asked to stop.
    pub(crate) fn stopped(&self, number: u32) -> bool {
        self.stop.load(Ordering::Acquire) >= number
    }

    /// The renderer's: voice `number` has ended here, played out or stopped.
    pub(crate) fn finish(&self, number: u32) {
        self.done.store(number, Ordering::Release);
    }

    /// The control side's: voice `number` has ended; when it is the last one
    /// sent, the slot is free.
    pub(crate) fn finished(&self, number: u32) -> bool {
        self.done.load(Ordering::Acquire) == number
    }

    /// The renderer's: the number of the voice waiting to start, if any.
    pub(crate) fn take_incoming_voice(&self) -> Option<u32> {
        // Read first, so a slot with nothing waiting is never written.
        if self.incoming_voice.load(Ordering::Relaxed) == 0 {
            return None;
        }
        match self.incoming_voice.swap(0, Ordering::Acquire) {
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
    pub fn play(&mut self) -> Voice<VOICES> {
        let free = (0..VOICES).find(|&slot| self.state.voices[slot].finished(self.sent[slot]));
        // Numbers only grow, so the smallest sent is the oldest voice.
        let oldest = || (0..VOICES).min_by_key(|&slot| self.sent[slot]);
        let slot = free
            .or_else(oldest)
            .expect("an audio has at least one voice");
        self.played += 1;
        self.sent[slot] = self.played;
        self.state.voices[slot].send_voice(self.played);
        Voice {
            state: Arc::clone(&self.state),
            slot,
            number: self.played,
            detached: false,
        }
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

/// One voice played from a loaded audio.
#[must_use = "dropping it stops the voice"]
pub struct Voice<const VOICES: usize> {
    state: Arc<AudioState<VOICES>>,
    /// Which of the audio's voice slots it plays in.
    slot: usize,
    /// Which voice of that slot it is.
    number: u32,
    /// Let go without stopping; the voice plays out.
    detached: bool,
}

impl<const VOICES: usize> Voice<VOICES> {
    /// Gives up the handle without stopping the voice, so it plays to its
    /// end; nothing can stop it after.
    pub fn detach(mut self) {
        self.detached = true;
    }
}

/// A voice plays while its handle is held, unless detached.
impl<const VOICES: usize> Drop for Voice<VOICES> {
    fn drop(&mut self) {
        if !self.detached {
            self.state.voices[self.slot].request_stop(self.number);
        }
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
        /// Which voice of its slot this is, so a stop for an older one is
        /// told apart.
        number: u32,
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
                if let Some(number) = slot.take_incoming_voice() {
                    *voice = Some(Voice {
                        number,
                        playhead: 0,
                    });
                }
                if let Some(playing) = voice
                    && slot.stopped(playing.number)
                {
                    slot.finish(playing.number);
                    *voice = None;
                }
                if let Some(playing) = voice {
                    playing.render(buffer, scratch);
                    add(out, scratch);
                    if playing.finished(buffer) {
                        slot.finish(playing.number);
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
