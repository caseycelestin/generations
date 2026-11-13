//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::mem;
use std::num::NonZeroUsize;

use rtrb::Producer;

use super::{Frame, Renderer};
use crate::audio::ChannelLayout;
use crate::engine::protocol::Return;
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd, VoiceId, VoiceSpec};

/// A voice while it plays.
#[derive(Debug)]
pub(crate) struct Voice {
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    /// Only moves the spec in, so nothing allocates.
    fn new(spec: VoiceSpec) -> Self {
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

    /// The one way a voice leaves: reports `why`, then hands its audio and
    /// report back to be freed on the control side, never here.
    fn end(self, why: VoiceEnd, to_control: &mut Producer<Return>) {
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
    /// one, so an `Add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
        }
    }

    /// Every playing voice.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.places.iter_mut().flatten()
    }

    /// Applies one voice command. Anything that leaves is handed back on
    /// `to_control`.
    pub(crate) fn apply(&mut self, command: VoiceCommand, to_control: &mut Producer<Return>) {
        match command {
            VoiceCommand::Add(spec) => self.add(Voice::new(spec), to_control),
            VoiceCommand::Remove(id) => self.remove(id, to_control),
        }
    }

    /// Takes a free place, or cuts the oldest voice for one.
    fn add(&mut self, voice: Voice, to_control: &mut Producer<Return>) {
        match self.places.iter_mut().find(|place| place.is_none()) {
            Some(free) => *free = Some(voice),
            None => {
                let oldest = self
                    .places
                    .iter_mut()
                    .flatten()
                    .min_by_key(|voice| voice.spec.id)
                    .expect("every place is taken, and there is at least one");
                let cut = mem::replace(oldest, voice);
                cut.end(VoiceEnd::Cut, to_control);
            }
        }
    }

    /// Ends the voice `id` as stopped, if it is still here.
    fn remove(&mut self, id: VoiceId, to_control: &mut Producer<Return>) {
        let found = self
            .places
            .iter_mut()
            .find(|place| matches!(place, Some(voice) if voice.spec.id == id));
        if let Some(voice) = found.and_then(Option::take) {
            voice.end(VoiceEnd::Stopped, to_control);
        }
    }
}
