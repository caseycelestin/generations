//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::mem;
use std::num::NonZeroUsize;

use rtrb::Producer;

use super::{Frame, Renderer};
use crate::engine::protocol::voice::{VoiceEnd, VoiceEvent, VoiceId, VoiceSpec};
use crate::engine::protocol::{BufferAddress, Event, HandleId, Message};
use crate::layout::ChannelLayout;

#[derive(Debug)]
pub(crate) struct Voice {
    id: HandleId,
    spec: VoiceSpec,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    pub(crate) fn new(id: HandleId, spec: VoiceSpec) -> Self {
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

    /// The one way a voice leaves.
    pub(crate) fn end(self, why: VoiceEnd, to_control: &mut Producer<Message<Event>>) {
        self.spec.report.end(why);
        let pushed = to_control.push(Message {
            id: self.id,
            data: VoiceEvent::Ended(self.spec).into(),
        });
        debug_assert!(pushed.is_ok(), "the ring back is sized never to fill");
    }
}

#[derive(Debug)]
pub(crate) struct Voices {
    /// `None` is free. Boxed so it cannot grow on the audio thread.
    places: Box<[Option<Voice>]>,
}

impl Voices {
    /// At least one, so `add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
        }
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

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.places.iter_mut().flatten()
    }

    pub(crate) fn remove(&mut self, id: VoiceId) -> Option<Voice> {
        let found = self
            .places
            .iter_mut()
            .find(|place| matches!(place, Some(voice) if voice.spec.id == id));
        found.and_then(Option::take)
    }

    /// Takes out every voice `test` picks, as `Vec::extract_if`: stopped
    /// early, the rest stay.
    pub(crate) fn extract_if(
        &mut self,
        mut test: impl FnMut(&mut Voice) -> bool,
    ) -> impl Iterator<Item = Voice> {
        self.places
            .iter_mut()
            .filter_map(move |place| place.take_if(|voice| test(voice)))
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = Voice> + '_ {
        self.places.iter_mut().filter_map(Option::take)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::engine::protocol::voice::VoiceReport;
    use crate::testing::buffer;

    fn voice(layout: ChannelLayout, samples: &[f32]) -> Voice {
        Voice::new(
            HandleId(0),
            VoiceSpec {
                id: VoiceId::next(),
                audio: Arc::new(buffer(layout, samples)),
                report: Arc::new(VoiceReport::new()),
            },
        )
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
    fn add_when_full_replaces_the_oldest() {
        let mut voices = voices(2);
        let oldest = voice(ChannelLayout::Mono, &[0.0]);
        let oldest_id = oldest.spec.id;
        voices.add(oldest);
        voices.add(voice(ChannelLayout::Mono, &[0.0]));
        let replaced = voices.add(voice(ChannelLayout::Mono, &[0.0]));
        assert_eq!(replaced.map(|voice| voice.spec.id), Some(oldest_id));
        assert_eq!(voices.iter_mut().count(), 2);
    }

    #[test]
    fn remove_takes_the_voice_out_once() {
        let mut voices = voices(1);
        let voice = voice(ChannelLayout::Mono, &[0.0]);
        let id = voice.spec.id;
        voices.add(voice);
        assert!(voices.remove(id).is_some());
        assert!(voices.remove(id).is_none());
    }

    #[test]
    fn extract_if_takes_only_the_voices_picked() {
        let mut voices = voices(2);
        let short = voice(ChannelLayout::Mono, &[0.0]);
        let short_id = short.spec.id;
        voices.add(short);
        voices.add(voice(ChannelLayout::Mono, &[0.0; 4]));
        let mut scratch = [[0.0; 2]; 2];
        for voice in voices.iter_mut() {
            voice.render(&mut scratch);
        }
        let extracted: Vec<VoiceId> = voices
            .extract_if(|voice| voice.finished())
            .map(|voice| voice.spec.id)
            .collect();
        assert_eq!(extracted, [short_id]);
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
