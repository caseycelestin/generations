//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::mem;
use std::num::NonZeroUsize;

use rtrb::Producer;

use super::{Frame, Renderer};
use crate::audio::ChannelLayout;
use crate::engine::protocol::Return;
use crate::engine::protocol::voice::{VoiceEnd, VoiceId, VoiceSpec};

/// A voice while it plays.
#[derive(Debug)]
pub(crate) struct Voice {
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    /// Only moves the spec in, so nothing allocates.
    pub(crate) fn new(spec: VoiceSpec) -> Self {
        Self { spec, position: 0 }
    }

    /// Writes its next frames into `scratch`, then silence once the audio
    /// runs out. Mono is heard at full level on both sides.
    pub(crate) fn render(&mut self, scratch: &mut [Frame]) {
        let audio = &self.spec.audio;
        let end = (self.position + scratch.len()).min(audio.frames());
        let samples = audio.frames_in(self.position..end);
        let (sound, silence) = scratch.split_at_mut(end - self.position);
        match audio.layout() {
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
        silence.fill([0.0; Renderer::CHANNELS]);
        self.position = end;
    }

    /// Its last frame has been rendered.
    fn finished(&self) -> bool {
        self.position == self.spec.audio.frames()
    }

    /// The one way a voice leaves: reports `why`, then hands its audio and
    /// report back to be freed on the control side, never here.
    pub(crate) fn end(self, why: VoiceEnd, to_control: &mut Producer<Return>) {
        self.spec.report.end(why);
        let pushed = to_control.push(Return::Voice {
            audio: self.spec.audio,
            report: self.spec.report,
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}

#[derive(Debug)]
pub(crate) struct Voices {
    /// A fixed number of places: `None` is free. A boxed slice cannot
    /// grow, so nothing here reallocates on the audio thread.
    places: Box<[Option<Voice>]>,
}

impl Voices {
    /// `count` free places, allocated here, on the control side. At least
    /// one, so `add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
        }
    }

    /// Every playing voice.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.places.iter_mut().flatten()
    }

    /// Takes out every voice that has played to its last frame.
    pub(crate) fn pop_finished(&mut self) -> impl Iterator<Item = Voice> + '_ {
        self.places
            .iter_mut()
            .filter_map(|place| place.take_if(|voice| voice.finished()))
    }

    /// Takes a free place, or the oldest voice's place, returning the
    /// voice replaced.
    pub(crate) fn add(&mut self, voice: Voice) -> Option<Voice> {
        match self.places.iter_mut().find(|place| place.is_none()) {
            Some(free) => {
                *free = Some(voice);
                None
            }
            None => {
                let oldest = self
                    .places
                    .iter_mut()
                    .flatten()
                    .min_by_key(|voice| voice.spec.id)
                    .expect("every place is taken, and there is at least one");
                Some(mem::replace(oldest, voice))
            }
        }
    }

    /// Takes out the voice `id`, if it is still here.
    pub(crate) fn remove(&mut self, id: VoiceId) -> Option<Voice> {
        let found = self
            .places
            .iter_mut()
            .find(|place| matches!(place, Some(voice) if voice.spec.id == id));
        found.and_then(Option::take)
    }
}
