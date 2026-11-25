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
        // Free once no `LoadedAudio` holds the state and the renderer has
        // dropped its clone of the buffer.
        let free = self.places.iter_mut().enumerate().find(|(_, place)| {
            Arc::strong_count(&place.state) == 1
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

pub(crate) fn create(count: usize) -> Audio {
    let states = (0..count).map(|_| Arc::new(AudioState {})).collect();
    // A place is refilled only after the renderer has taken its last
    // message, so a ring of one per place is never full.
    let (producer, _consumer) = RingBuffer::new(count);
    Audio::new(states, producer)
}
