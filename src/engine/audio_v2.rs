use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::engine::audio::AudioBuffer;

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
}

#[derive(Debug)]
struct VoiceState {
    /// The voice in this slot, by its audio's play count, or 0 before any;
    /// a `Voice` stops it only while this is still its number.
    number: AtomicU64,
    /// A `VoiceLifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
}

#[derive(Debug)]
pub(crate) struct AudioState {
    /// A `Lifecycle` as its `u8`, since no atomic holds an enum.
    lifecycle: AtomicU8,
    voices: Box<[VoiceState]>,
}

#[derive(Debug)]
pub(crate) struct Voice {
    state: Arc<AudioState>,
    /// Which of the audio's voices it plays in.
    slot: usize,
    /// Which voice of that slot it is.
    number: u64,
}

#[derive(Debug)]
#[must_use = "dropping it unloads the audio"]
pub(crate) struct LoadedAudio {
    state: Arc<AudioState>,
    /// Voices started so far, by this handle alone; the next one's number
    /// is this plus one, so none is 0 and the smallest is the oldest.
    played: u64,
}

impl LoadedAudio {
    pub(crate) fn play(&mut self) -> Voice {
        let voices = &self.state.voices;
        // Acquire: pairs with the renderer's store when a voice ends.
        let free = voices.iter().position(|voice| {
            voice.lifecycle.load(Ordering::Acquire) == VoiceLifecycle::Idle as u8
        });
        // With none free, the oldest is cut, since a press must sound.
        let oldest = || {
            (0..voices.len())
                // Relaxed: only this handle writes it.
                .min_by_key(|&index| voices[index].number.load(Ordering::Relaxed))
                .expect("an audio has at least one voice")
        };
        let slot = free.unwrap_or_else(oldest);
        self.played += 1;
        let voice = &self.state.voices[slot];
        // Relaxed: the store below publishes it.
        voice.number.store(self.played, Ordering::Relaxed);
        // Release: the renderer sees the new number with it. A plain store,
        // even over a playing voice: the renderer leaves a state only by
        // compare_exchange, so it never overwrites this.
        voice
            .lifecycle
            .store(VoiceLifecycle::StartRequested as u8, Ordering::Release);
        Voice {
            state: Arc::clone(&self.state),
            slot,
            number: self.played,
        }
    }
}

impl Drop for LoadedAudio {
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

/// Nothing was loaded; the buffer comes back in it.
#[derive(Debug, Error)]
pub(crate) enum LoadError {
    #[error("every place for loaded audio is held")]
    Full(AudioBuffer),
}

/// Only the control side has it.
#[derive(Debug)]
struct Place {
    state: Arc<AudioState>,
    /// The control side's clone, so the renderer's is never the last.
    buffer: Option<Arc<AudioBuffer>>,
}

#[derive(Debug)]
pub(crate) struct Audio {
    places: Vec<Place>,
    producer: Producer<(usize, Arc<AudioBuffer>)>,
}

impl Audio {
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

    pub(crate) fn load(&mut self, buffer: AudioBuffer) -> Result<LoadedAudio, LoadError> {
        // Free once no `LoadedAudio` holds the state, so only the two sides'
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
            return Err(LoadError::Full(buffer));
        };
        let buffer = Arc::new(buffer);
        // Drops the old buffer here, on the control side.
        free.buffer = Some(Arc::clone(&buffer));
        self.producer
            .push((index, buffer))
            .expect("the ring holds one message per place");
        Ok(LoadedAudio {
            state: Arc::clone(&free.state),
            played: 0,
        })
    }
}

pub(crate) fn create(count: usize) -> (Audio, realtime::Audio) {
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
    let realtime = realtime::Audio::new(states.clone(), consumer);
    (Audio::new(states, producer), realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Consumer;

    use super::{AudioState, Lifecycle};
    use crate::engine::audio::AudioBuffer;

    #[derive(Debug)]
    struct Place {
        state: Arc<AudioState>,
        /// Dropped on unload, never freed here: the control side keeps a
        /// clone.
        buffer: Option<Arc<AudioBuffer>>,
    }

    #[derive(Debug)]
    pub(crate) struct Audio {
        /// In `create`'s order, so a message's index names the same place
        /// on both sides; never resized.
        places: Vec<Place>,
        consumer: Consumer<(usize, Arc<AudioBuffer>)>,
    }

    impl Audio {
        pub(super) fn new(
            states: Vec<Arc<AudioState>>,
            consumer: Consumer<(usize, Arc<AudioBuffer>)>,
        ) -> Self {
            let places = states
                .into_iter()
                .map(|state| Place {
                    state,
                    buffer: None,
                })
                .collect();
            Self { places, consumer }
        }

        /// Takes in every buffer loaded since the last block.
        pub(crate) fn receive(&mut self) {
            while let Ok((index, buffer)) = self.consumer.pop() {
                let place = &mut self.places[index];
                // The control side refills a place only once this side has
                // dropped its buffer, so nothing is dropped here.
                debug_assert!(place.buffer.is_none(), "a loaded place is refilled");
                place.buffer = Some(buffer);
                // Fails if the handle was dropped before its buffer arrived,
                // leaving the unload it asked for. Relaxed: nothing is
                // published through it.
                let _ = place.state.lifecycle.compare_exchange(
                    Lifecycle::Idle as u8,
                    Lifecycle::Loaded as u8,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }

        /// Drops every unloaded buffer and frees its place. After `receive`,
        /// so one unloaded before it arrived goes in the same block.
        pub(crate) fn unload(&mut self) {
            for place in &mut self.places {
                let lifecycle = place.state.lifecycle.load(Ordering::Relaxed);
                if lifecycle != Lifecycle::UnloadRequested as u8 {
                    continue;
                }
                // Never the last clone: the control side keeps one.
                place.buffer = None;
                // Release: the control side sees the buffer dropped before it
                // sees the place free.
                place
                    .state
                    .lifecycle
                    .store(Lifecycle::Idle as u8, Ordering::Release);
            }
        }
    }
}
