//! Voices on the render side: each one while it plays, and the places
//! they live.

use std::mem;
use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;

use super::{Frame, Renderer};
use crate::engine::protocol::voice::VoiceData;
use crate::layout::ChannelLayout;
use crate::transfer::{Completion, RealtimeEnd};

#[derive(Debug)]
pub(crate) struct Voice {
    data: Completion<VoiceData>,
    /// The next frame to play.
    position: usize,
}

impl Voice {
    pub(crate) fn new(data: Completion<VoiceData>) -> Self {
        Self { data, position: 0 }
    }

    /// Silence once the audio runs out. Mono goes to both sides at full
    /// level.
    pub(crate) fn render(&mut self, scratch: &mut [Frame]) {
        let audio = &self.data.audio;
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
        silence.fill([0.0; Renderer::OUTPUT_CHANNELS]);
        self.position = end;
    }

    /// Its handle asked it to stop.
    pub(crate) fn stopped(&self) -> bool {
        self.data.state.stop.load(Ordering::Relaxed)
    }

    /// Its last frame has been rendered.
    pub(crate) fn finished(&self) -> bool {
        self.position == self.data.audio.frames()
    }

    /// The one way a voice leaves: its data goes back whole, to be freed
    /// on the control side.
    pub(crate) fn end(self, transfers: &mut RealtimeEnd<VoiceData>) {
        transfers.complete(self.data);
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
    /// The only way voices arrive or leave.
    transfers: RealtimeEnd<VoiceData>,
}

impl Voices {
    /// At least one, so `add` always has a place or an oldest to cut.
    pub(crate) fn new(count: NonZeroUsize, transfers: RealtimeEnd<VoiceData>) -> Self {
        Self {
            places: (0..count.get()).map(|_| None).collect(),
            added: 0,
            transfers,
        }
    }

    /// New voices join, cutting the oldest when every place is taken;
    /// stopped ones end.
    pub(crate) fn receive(&mut self) {
        while let Some(data) = self.transfers.try_recv() {
            if let Some(cut) = self.add(Voice::new(data)) {
                cut.end(&mut self.transfers);
            }
        }
        for stopped in take_if(&mut self.places, |voice| voice.stopped()) {
            stopped.end(&mut self.transfers);
        }
    }

    /// Voices whose audio has run out end.
    pub(crate) fn end_finished(&mut self) {
        for finished in take_if(&mut self.places, |voice| voice.finished()) {
            finished.end(&mut self.transfers);
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
}

/// Takes out every voice `test` picks, as `Vec::extract_if`: stopped early,
/// the rest stay. On the places alone, so the end can be borrowed beside it.
fn take_if(
    places: &mut [Option<(u64, Voice)>],
    mut test: impl FnMut(&mut Voice) -> bool,
) -> impl Iterator<Item = Voice> {
    places
        .iter_mut()
        .filter_map(move |place| place.take_if(|(_, voice)| test(voice)))
        .map(|(_, voice)| voice)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::engine::protocol::voice::VoiceState;
    use crate::testing::buffer;
    use crate::transfer;

    /// Received as the renderer receives it. Tests tell voices apart by
    /// their length.
    fn voice(layout: ChannelLayout, samples: &[f32]) -> Voice {
        let (mut control, mut realtime) = transfer::channel(1);
        let audio = Arc::new(buffer(layout, samples));
        let state = Arc::new(VoiceState::default());
        let _transfer = control.try_start(VoiceData { audio, state }).unwrap();
        Voice::new(realtime.try_recv().unwrap())
    }

    fn frames(voice: &Voice) -> usize {
        voice.data.audio.frames()
    }

    fn voices(count: usize) -> Voices {
        let (_, realtime) = transfer::channel(0);
        Voices::new(NonZeroUsize::new(count).unwrap(), realtime)
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
        voices.add(voice(ChannelLayout::Mono, &[0.0; 1]));
        voices.add(voice(ChannelLayout::Mono, &[0.0; 2]));
        let newest = voice(ChannelLayout::Mono, &[0.0; 3]);
        let replaced = voices.add(newest);
        assert!(replaced.is_some_and(|voice| frames(&voice) == 1));
        assert_eq!(voices.iter_mut().count(), 2);
    }

    #[test]
    fn take_if_takes_only_the_voices_picked() {
        let mut voices = voices(2);
        voices.add(voice(ChannelLayout::Mono, &[0.0]));
        voices.add(voice(ChannelLayout::Mono, &[0.0; 4]));
        let mut scratch = [[0.0; 2]; 2];
        for voice in voices.iter_mut() {
            voice.render(&mut scratch);
        }
        let taken: Vec<usize> = take_if(&mut voices.places, |voice| voice.finished())
            .map(|voice| frames(&voice))
            .collect();
        assert_eq!(taken, [1]);
        assert_eq!(voices.iter_mut().count(), 1);
    }
}
