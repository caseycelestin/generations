use std::sync::Arc;

use rtrb::{Producer, RingBuffer};
use thiserror::Error;

use crate::engine::audio::AudioBuffer;

#[derive(Debug)]
pub(crate) struct AudioState {}

#[derive(Debug)]
pub(crate) struct LoadedAudio {
    state: Arc<AudioState>,
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
            Arc::strong_count(&place.state) == 2
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
        })
    }
}

pub(crate) fn create(count: usize) -> (Audio, realtime::Audio) {
    let states: Vec<Arc<AudioState>> = (0..count).map(|_| Arc::new(AudioState {})).collect();
    // A place is refilled only after the renderer has taken its last
    // message, so a ring of one per place is never full.
    let (producer, consumer) = RingBuffer::new(count);
    let realtime = realtime::Audio::new(states.clone(), consumer);
    (Audio::new(states, producer), realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;

    use rtrb::Consumer;

    use super::AudioState;
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
            }
        }
    }
}
