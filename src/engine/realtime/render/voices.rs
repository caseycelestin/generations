//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::mem;
use std::num::NonZeroUsize;
use std::sync::Arc;

use super::{Frame, Renderer};
use crate::audio::AudioBuffer;
use crate::collector::Disposer;
use crate::duplex::{Id, RealtimeEnd};
use crate::engine::protocol::voice::{VoiceEnd, VoiceEvent, VoiceSpec, VoiceState};
use crate::engine::protocol::{BufferAddress, Command, Event};
use crate::layout::ChannelLayout;

#[derive(Debug)]
pub(crate) struct Voice {
    id: Id,
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    pub(crate) fn new(id: Id, spec: VoiceSpec) -> Self {
        Self {
            id,
            spec,
            position: 0,
        }
    }

    /// Silence once the audio runs out. Mono goes to both sides at full
    /// level.
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

    pub(crate) fn plays(&self, audio: BufferAddress) -> bool {
        BufferAddress::of(&self.spec.audio) == audio
    }

    /// Its last frame has been rendered.
    pub(crate) fn finished(&self) -> bool {
        self.position == self.spec.audio.frames()
    }

    /// The one way a voice leaves. Its audio goes to the collector before
    /// its end goes out, so a voice reported ended has let go of it.
    pub(crate) fn end(
        self,
        why: VoiceEnd,
        channel: &mut RealtimeEnd<Command, Event>,
        disposer: &mut Disposer<Arc<AudioBuffer>>,
    ) {
        let disposed = disposer.dispose(self.spec.audio);
        debug_assert!(disposed.is_ok(), "the collector is sized for every voice");
        let event = VoiceEvent {
            state: VoiceState::Ended(why),
        };
        let ended = channel.end(self.id, event);
        debug_assert!(ended.is_ok(), "a voice reserves its end");
    }
}

#[derive(Debug)]
pub(crate) struct Voices {
    /// `None` is free; a taken place holds its voice's number in the order
    /// voices were added, lower being older. Boxed so it cannot grow on the
    /// audio thread.
    places: Box<[Option<(u64, Voice)>]>,
    /// How many voices have been added, so the next one's number. A `u64`
    /// cannot run out: a million a second lasts half a million years.
    added: u64,
}

impl Voices {
    /// At least one, so `add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
            added: 0,
        }
    }

    /// Takes a free place, or the oldest voice's place, returning the
    /// voice replaced.
    pub(crate) fn add(&mut self, voice: Voice) -> Option<Voice> {
        let numbered = (self.added, voice);
        self.added += 1;
        match self.places.iter_mut().find(|place| place.is_none()) {
            Some(free) => {
                *free = Some(numbered);
                None
            }
            None => {
                let oldest = self
                    .places
                    .iter_mut()
                    .flatten()
                    .min_by_key(|(number, _)| *number)
                    .expect("every place is taken, and there is at least one");
                let (_, replaced) = mem::replace(oldest, numbered);
                Some(replaced)
            }
        }
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.places.iter_mut().flatten().map(|(_, voice)| voice)
    }

    pub(crate) fn remove(&mut self, id: Id) -> Option<Voice> {
        let found = self
            .places
            .iter_mut()
            .find(|place| matches!(place, Some((_, voice)) if voice.id == id));
        let (_, voice) = found?.take()?;
        Some(voice)
    }

    /// Takes out every voice `test` picks, as `Vec::extract_if`: stopped
    /// early, the rest stay.
    pub(crate) fn extract_if(
        &mut self,
        mut test: impl FnMut(&mut Voice) -> bool,
    ) -> impl Iterator<Item = Voice> {
        self.places
            .iter_mut()
            .filter_map(move |place| place.take_if(|(_, voice)| test(voice)))
            .map(|(_, voice)| voice)
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = Voice> + '_ {
        self.places
            .iter_mut()
            .filter_map(Option::take)
            .map(|(_, voice)| voice)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::testing::buffer;

    fn voice(layout: ChannelLayout, samples: &[f32]) -> Voice {
        voice_from(Id(0), layout, samples)
    }

    /// For tests that tell voices apart by their sender.
    fn voice_from(id: Id, layout: ChannelLayout, samples: &[f32]) -> Voice {
        let audio = Arc::new(buffer(layout, samples));
        Voice::new(id, VoiceSpec { audio })
    }

    fn voices(count: usize) -> Voices {
        Voices::new(NonZeroUsize::new(count).unwrap())
    }

    #[test]
    fn mono_is_heard_on_both_sides() {
        let mut voice = voice(ChannelLayout::Mono, &[0.1, 0.2]);
        let mut scratch = [[0.0; 2]; 2];
        voice.render(&mut scratch);
        assert_eq!(scratch, [[0.1, 0.1], [0.2, 0.2]]);
    }

    #[test]
    fn stereo_is_heard_as_stored() {
        let mut voice = voice(ChannelLayout::Stereo, &[0.1, 0.2, 0.3, 0.4]);
        let mut scratch = [[0.0; 2]; 2];
        voice.render(&mut scratch);
        assert_eq!(scratch, [[0.1, 0.2], [0.3, 0.4]]);
    }

    #[test]
    fn silence_follows_the_last_frame() {
        let mut voice = voice(ChannelLayout::Mono, &[0.5]);
        let mut scratch = [[9.0; 2]; 3];
        voice.render(&mut scratch);
        assert_eq!(scratch, [[0.5, 0.5], [0.0, 0.0], [0.0, 0.0]]);
    }

    #[test]
    fn each_render_continues_where_the_last_stopped() {
        let mut voice = voice(ChannelLayout::Mono, &[1.0, 2.0, 3.0, 4.0]);
        let mut scratch = [[0.0; 2]; 2];
        voice.render(&mut scratch);
        voice.render(&mut scratch);
        assert_eq!(scratch, [[3.0, 3.0], [4.0, 4.0]]);
    }

    #[test]
    fn a_voice_plays_only_its_own_audio() {
        let voice = voice(ChannelLayout::Mono, &[0.0]);
        let other = Arc::new(buffer(ChannelLayout::Mono, &[0.0]));
        assert!(voice.plays(BufferAddress::of(&voice.spec.audio)));
        assert!(!voice.plays(BufferAddress::of(&other)));
    }

    #[test]
    fn a_voice_is_finished_once_its_last_frame_renders() {
        let mut voice = voice(ChannelLayout::Mono, &[0.0; 4]);
        let mut scratch = [[0.0; 2]; 2];
        voice.render(&mut scratch);
        assert!(!voice.finished());
        voice.render(&mut scratch);
        assert!(voice.finished());
    }

    #[test]
    fn add_takes_a_free_place() {
        let mut voices = voices(2);
        assert!(voices.add(voice(ChannelLayout::Mono, &[0.0])).is_none());
        assert!(voices.add(voice(ChannelLayout::Mono, &[0.0])).is_none());
        assert_eq!(voices.iter_mut().count(), 2);
    }

    #[test]
    fn add_when_full_replaces_the_first_added() {
        let mut voices = voices(2);
        voices.add(voice_from(Id(1), ChannelLayout::Mono, &[0.0]));
        voices.add(voice_from(Id(0), ChannelLayout::Mono, &[0.0]));
        let newest = voice_from(Id(2), ChannelLayout::Mono, &[0.0]);
        let replaced = voices.add(newest);
        assert_eq!(replaced.map(|voice| voice.id), Some(Id(1)));
        assert_eq!(voices.iter_mut().count(), 2);
    }

    #[test]
    fn remove_takes_the_voice_out_once() {
        let mut voices = voices(1);
        voices.add(voice_from(Id(0), ChannelLayout::Mono, &[0.0]));
        assert!(voices.remove(Id(0)).is_some());
        assert!(voices.remove(Id(0)).is_none());
    }

    #[test]
    fn extract_if_takes_only_the_voices_picked() {
        let mut voices = voices(2);
        voices.add(voice_from(Id(0), ChannelLayout::Mono, &[0.0]));
        voices.add(voice_from(Id(1), ChannelLayout::Mono, &[0.0; 4]));
        let mut scratch = [[0.0; 2]; 2];
        for voice in voices.iter_mut() {
            voice.render(&mut scratch);
        }
        let extracted: Vec<Id> = voices
            .extract_if(|voice| voice.finished())
            .map(|voice| voice.id)
            .collect();
        assert_eq!(extracted, [Id(0)]);
        assert_eq!(voices.iter_mut().count(), 1);
    }

    #[test]
    fn drain_takes_out_every_voice() {
        let mut voices = voices(3);
        voices.add(voice(ChannelLayout::Mono, &[0.0]));
        voices.add(voice(ChannelLayout::Mono, &[0.0]));
        assert_eq!(voices.drain().count(), 2);
        assert_eq!(voices.iter_mut().count(), 0);
    }
}
