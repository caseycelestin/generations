use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::layout::ChannelLayout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Lifecycle {
    /// Free for `load`.
    Idle,
    /// The renderer has its buffer.
    Loaded,
    /// Its handle is gone; only the renderer moves it on, to `Idle`, once
    /// it has dropped its buffer.
    UnloadRequested,
}

impl Lifecycle {
    /// Only ever given what was stored from a `Lifecycle`.
    fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Idle as u8 => Self::Idle,
            v if v == Self::Loaded as u8 => Self::Loaded,
            v if v == Self::UnloadRequested as u8 => Self::UnloadRequested,
            _ => unreachable!("only a Lifecycle is stored"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum VoiceLifecycle {
    /// Free for `play`.
    Idle,
    /// `play` asked; the renderer starts it from the top at its next block,
    /// cutting whatever was playing there.
    StartRequested,
    /// The renderer is playing it.
    Playing,
    /// Its `Voice` was dropped; the renderer ends it at its next block.
    StopRequested,
}

impl VoiceLifecycle {
    /// Bits it takes in a `VoiceState`: four variants, full. A fifth needs
    /// a third bit.
    const BITS: u32 = 2;

    /// Only ever given what was stored from a `VoiceLifecycle`.
    fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Idle as u8 => Self::Idle,
            v if v == Self::StartRequested as u8 => Self::StartRequested,
            v if v == Self::Playing as u8 => Self::Playing,
            v if v == Self::StopRequested as u8 => Self::StopRequested,
            _ => unreachable!("only a VoiceLifecycle is stored"),
        }
    }
}

#[derive(Debug)]
struct VoiceState {
    /// The voice's number above its `VoiceLifecycle`, in one atomic so a
    /// `Voice` can check the slot is still its voice and change it in one
    /// step. The number is its audio's play count, or 0 before any.
    packed: AtomicU64,
}

impl VoiceState {
    fn pack(number: u64, lifecycle: VoiceLifecycle) -> u64 {
        (number << VoiceLifecycle::BITS) | lifecycle as u64
    }

    fn unpack(packed: u64) -> (u64, VoiceLifecycle) {
        let mask = (1 << VoiceLifecycle::BITS) - 1;
        let lifecycle = VoiceLifecycle::from_u8((packed & mask) as u8);
        (packed >> VoiceLifecycle::BITS, lifecycle)
    }

    fn load(&self, ordering: Ordering) -> (u64, VoiceLifecycle) {
        Self::unpack(self.packed.load(ordering))
    }

    /// Cuts whatever was playing here. Release: the renderer sees the new
    /// number with the request. A plain store: the renderer leaves a state
    /// only by compare_exchange, so it never overwrites this.
    fn set_start_requested(&self, number: u64) {
        let packed = Self::pack(number, VoiceLifecycle::StartRequested);
        self.packed.store(packed, Ordering::Release);
    }

    /// The renderer's: voice `number`, asked to start, is now playing.
    /// False if `play` or a stop got there first; the renderer sees that
    /// next block. Relaxed: the renderer's Acquire load saw the request.
    fn set_playing(&self, number: u64) -> bool {
        let requested = Self::pack(number, VoiceLifecycle::StartRequested);
        let playing = Self::pack(number, VoiceLifecycle::Playing);
        self.packed
            .compare_exchange(requested, playing, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// The renderer's: voice `number` has ended, played out or stopped, so
    /// its slot is free. Only while the slot still holds that voice, so a
    /// `play` that took it meanwhile is never lost. Release: pairs with
    /// `play`'s Acquire load.
    fn set_idle(&self, number: u64) {
        let _ = self
            .packed
            .try_update(Ordering::Release, Ordering::Relaxed, |packed| {
                let (current, lifecycle) = Self::unpack(packed);
                if current != number {
                    return None;
                }
                let next = match lifecycle {
                    VoiceLifecycle::Playing | VoiceLifecycle::StopRequested => VoiceLifecycle::Idle,
                    VoiceLifecycle::Idle | VoiceLifecycle::StartRequested => return None,
                };
                Some(Self::pack(number, next))
            });
    }

    /// The renderer's, once its audio is unloaded: frees the slot whatever it
    /// held. No `play` can race it then, since the `Audio` is gone.
    /// Release: pairs with `play`'s Acquire load.
    fn set_unloaded(&self) {
        let packed = Self::pack(0, VoiceLifecycle::Idle);
        self.packed.store(packed, Ordering::Release);
    }

    /// Only while the slot still holds voice `number`, so a stale `Voice`
    /// never stops a newer one. Relaxed: nothing is published through it.
    fn set_stop_requested(&self, number: u64) {
        let _ = self
            .packed
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |packed| {
                let (current, lifecycle) = Self::unpack(packed);
                if current != number {
                    return None;
                }
                let next = match lifecycle {
                    VoiceLifecycle::StartRequested | VoiceLifecycle::Playing => {
                        VoiceLifecycle::StopRequested
                    }
                    VoiceLifecycle::Idle | VoiceLifecycle::StopRequested => return None,
                };
                Some(Self::pack(number, next))
            });
    }
}

/// The control side keeps a clone, so the renderer never frees it.
#[derive(Debug)]
pub(crate) struct AudioBuffer {
    pub(crate) samples: Vec<f32>,
    pub(crate) layout: ChannelLayout,
}

#[derive(Debug)]
struct AudioState {
    /// A `Lifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
    voices: Box<[VoiceState]>,
}

#[derive(Debug)]
#[must_use = "dropping it stops the voice"]
pub struct Voice {
    state: Arc<AudioState>,
    /// Which of the audio's voices it plays in.
    slot: usize,
    /// Which voice of that slot it is.
    number: u64,
    /// Let go without stopping; the voice plays out.
    detached: bool,
}

impl Voice {
    /// Gives up the handle without stopping the voice, so it plays to its
    /// end; nothing can stop it after.
    pub fn detach(mut self) {
        // A flag, not `mem::forget`: forgetting would keep the state's
        // count up, so its place could never be loaded again.
        self.detached = true;
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        if !self.detached {
            self.state.voices[self.slot].set_stop_requested(self.number);
        }
    }
}

#[derive(Debug)]
#[must_use = "dropping it unloads the audio"]
pub struct Audio {
    state: Arc<AudioState>,
    /// Voices started so far, by this handle alone; the next one's number
    /// is this plus one, so none is 0 and the smallest is the oldest.
    played: u64,
}

impl Audio {
    pub fn play(&mut self) -> Voice {
        let voices = &self.state.voices;
        // Acquire: pairs with the renderer's store when a voice ends.
        let free = voices.iter().position(|voice| {
            let (_, lifecycle) = voice.load(Ordering::Acquire);
            lifecycle == VoiceLifecycle::Idle
        });
        // With none free, the oldest is cut, since a press must sound.
        let oldest = || {
            (0..voices.len())
                // Relaxed: only this handle writes numbers.
                .min_by_key(|&index| voices[index].load(Ordering::Relaxed).0)
                .expect("an audio has at least one voice")
        };
        let slot = free.unwrap_or_else(oldest);
        self.played += 1;
        self.state.voices[slot].set_start_requested(self.played);
        Voice {
            state: Arc::clone(&self.state),
            slot,
            number: self.played,
            detached: false,
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        // Retried if the renderer moves it meanwhile, since its buffer may
        // not have arrived yet. Relaxed: nothing is published through it.
        let lifecycle = &self.state.lifecycle;
        let _ = lifecycle.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            let next = match Lifecycle::from_u8(current) {
                Lifecycle::Idle | Lifecycle::Loaded => Lifecycle::UnloadRequested,
                Lifecycle::UnloadRequested => return None,
            };
            Some(next as u8)
        });
    }
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

/// Only the control side has it.
#[derive(Debug)]
struct Place {
    state: Arc<AudioState>,
    /// The control side's clone, so the renderer's is never the last.
    buffer: Option<Arc<AudioBuffer>>,
}

#[derive(Debug)]
pub struct AudioPool {
    places: Box<[Place]>,
    producer: Producer<(usize, Arc<AudioBuffer>)>,
}

impl AudioPool {
    fn new(states: Vec<Arc<AudioState>>, producer: Producer<(usize, Arc<AudioBuffer>)>) -> Self {
        let places = states
            .into_iter()
            .map(|state| Place {
                state,
                buffer: None,
            })
            .collect();
        Self { places, producer }
    }

    pub fn load(&mut self, samples: Vec<f32>, layout: ChannelLayout) -> Result<Audio, LoadError> {
        if !samples.len().is_multiple_of(layout.count()) {
            return Err(LoadError::PartFrame(samples));
        }
        // Free once no `Audio` holds the state, so only the two sides'
        // lists do, and the renderer has dropped its clone of the buffer.
        let free = self.places.iter_mut().enumerate().find(|(_, place)| {
            // Acquire: pairs with the store that frees the place.
            place.state.lifecycle.load(Ordering::Acquire) == Lifecycle::Idle as u8
                && Arc::strong_count(&place.state) == 2
                && place
                    .buffer
                    .as_ref()
                    .is_none_or(|buffer| Arc::strong_count(buffer) == 1)
        });
        let Some((index, free)) = free else {
            return Err(LoadError::Full(samples));
        };
        let buffer = Arc::new(AudioBuffer { samples, layout });
        // Drops the old buffer here, on the control side.
        free.buffer = Some(Arc::clone(&buffer));
        self.producer
            .push((index, buffer))
            .expect("the ring holds one message per place");
        Ok(Audio {
            state: Arc::clone(&free.state),
            played: 0,
        })
    }
}

pub(crate) fn create(count: usize) -> (AudioPool, realtime::AudioPool) {
    let states: Vec<Arc<AudioState>> = (0..count)
        .map(|_| {
            Arc::new(AudioState {
                lifecycle: AtomicU8::new(Lifecycle::Idle as u8),
                voices: Box::new([]),
            })
        })
        .collect();
    // A place is refilled only after the renderer has taken its last
    // message, so a ring of one per place is never full.
    let (producer, consumer) = RingBuffer::new(count);
    let realtime = realtime::AudioPool::new(states.clone(), consumer);
    (AudioPool::new(states, producer), realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Consumer;

    use super::AudioBuffer;
    use super::{AudioState, Lifecycle, VoiceLifecycle};
    use crate::engine::renderer::Frame;
    use crate::layout::ChannelLayout;

    #[derive(Debug)]
    struct Voice {
        state: Arc<AudioState>,
        /// Which of the state's voices it is.
        slot: usize,
        /// Dropped on unload, never freed here: the control side keeps a
        /// clone.
        buffer: Option<Arc<AudioBuffer>>,
        /// The next frame, from the start of the buffer; only the renderer
        /// uses it, so no atomic.
        playhead: usize,
    }

    impl Voice {
        /// Drops its buffer, never the last clone, and frees its slot.
        fn unload(&mut self) {
            self.buffer = None;
            self.state.voices[self.slot].set_unloaded();
        }

        /// Acts on what voice `number` was asked; true if it plays this
        /// block.
        fn update_lifecycle(&mut self, number: u64, lifecycle: VoiceLifecycle) -> bool {
            let voice = &self.state.voices[self.slot];
            match lifecycle {
                VoiceLifecycle::Idle => false,
                VoiceLifecycle::StopRequested => {
                    voice.set_idle(number);
                    false
                }
                VoiceLifecycle::StartRequested => {
                    // Lost to a newer play or a stop; seen next block.
                    if !voice.set_playing(number) {
                        return false;
                    }
                    self.playhead = 0;
                    true
                }
                VoiceLifecycle::Playing => true,
            }
        }

        fn render(&mut self, scratch: &mut [Frame]) {
            // Acquire: pairs with `play`'s Release store, so a new voice's
            // number comes with its request.
            let (number, lifecycle) = self.state.voices[self.slot].load(Ordering::Acquire);
            if !self.update_lifecycle(number, lifecycle) {
                return;
            }
            let Some(buffer) = &self.buffer else {
                return;
            };
            let channels = buffer.layout.count();
            let frames = buffer.samples.len() / channels;
            // Past the buffer's end it adds nothing.
            let end = (self.playhead + scratch.len()).min(frames);
            let samples = &buffer.samples[self.playhead * channels..end * channels];
            match buffer.layout {
                // Both sides at full level.
                ChannelLayout::Mono => {
                    for (frame, &sample) in scratch.iter_mut().zip(samples) {
                        frame[0] += sample;
                        frame[1] += sample;
                    }
                }
                ChannelLayout::Stereo => {
                    let (pairs, _) = samples.as_chunks::<2>();
                    for (frame, pair) in scratch.iter_mut().zip(pairs) {
                        frame[0] += pair[0];
                        frame[1] += pair[1];
                    }
                }
            }
            self.playhead = end;
            if end == frames {
                self.state.voices[self.slot].set_idle(number);
            }
        }
    }

    #[derive(Debug)]
    struct Audio {
        state: Arc<AudioState>,
        /// Dropped on unload, never freed here: the control side keeps a
        /// clone.
        buffer: Option<Arc<AudioBuffer>>,
        /// By slot, like the state's voices.
        voices: Box<[Voice]>,
    }

    impl Audio {
        /// Allocates its voices, so only at `create`.
        fn new(state: Arc<AudioState>) -> Self {
            let voices = (0..state.voices.len())
                .map(|slot| Voice {
                    state: Arc::clone(&state),
                    slot,
                    buffer: None,
                    playhead: 0,
                })
                .collect();
            Self {
                state,
                buffer: None,
                voices,
            }
        }

        fn load(&mut self, buffer: Arc<AudioBuffer>) {
            // Cloning only counts; nothing is allocated here.
            for voice in &mut self.voices {
                voice.buffer = Some(Arc::clone(&buffer));
            }
            self.buffer = Some(buffer);
            // Fails if the handle was dropped before its buffer arrived,
            // leaving the unload it asked for. Relaxed: nothing is published
            // through it.
            let _ = self.state.lifecycle.compare_exchange(
                Lifecycle::Idle as u8,
                Lifecycle::Loaded as u8,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }

        /// Drops its buffer and frees its voices, if its handle asked.
        fn unload(&mut self) {
            let lifecycle = self.state.lifecycle.load(Ordering::Relaxed);
            if lifecycle != Lifecycle::UnloadRequested as u8 {
                return;
            }
            for voice in &mut self.voices {
                voice.unload();
            }
            // Never the last clone: the control side keeps one.
            self.buffer = None;
            // Release: the control side sees the buffers dropped and the
            // voices free before it sees the audio free.
            self.state
                .lifecycle
                .store(Lifecycle::Idle as u8, Ordering::Release);
        }

        fn render(&mut self, scratch: &mut [Frame]) {
            // Every voice is idle while there is no buffer: unload frees them.
            if self.buffer.is_none() {
                return;
            }
            for voice in &mut self.voices {
                voice.render(scratch);
            }
        }
    }

    #[derive(Debug)]
    pub(crate) struct AudioPool {
        /// In `create`'s order, so a message's index names the same audio
        /// on both sides; never resized.
        audio: Box<[Audio]>,
        consumer: Consumer<(usize, Arc<AudioBuffer>)>,
    }

    impl AudioPool {
        pub(super) fn new(
            states: Vec<Arc<AudioState>>,
            consumer: Consumer<(usize, Arc<AudioBuffer>)>,
        ) -> Self {
            let audio = states.into_iter().map(Audio::new).collect();
            Self { audio, consumer }
        }

        /// Takes in every buffer loaded since the last block.
        pub(crate) fn receive(&mut self) {
            while let Ok((index, buffer)) = self.consumer.pop() {
                let audio = &mut self.audio[index];
                // The control side refills an audio only once this side has
                // dropped its buffer, so nothing is dropped here.
                debug_assert!(audio.buffer.is_none(), "a loaded audio is refilled");
                audio.load(buffer);
            }
        }

        /// Drops every unloaded buffer and frees its audio. After `receive`,
        /// so one unloaded before it arrived goes in the same block.
        pub(crate) fn unload(&mut self) {
            for audio in &mut self.audio {
                audio.unload();
            }
        }

        pub(crate) fn render(&mut self, scratch: &mut [Frame]) {
            for audio in &mut self.audio {
                audio.render(scratch);
            }
        }
    }
}
