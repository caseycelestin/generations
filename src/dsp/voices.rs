//! The sounds playing right now: a fixed set of voices.
//!
//! A [`Voice`] is one sound, a [`Playhead`] with the [`VoiceId`] the engine
//! gave it. [`Voices`] is the fixed array of them: as many sounds can be
//! heard at once as there are voices, and a press that finds them all
//! taken steals the oldest. Its methods are the whole policy: start a
//! sound, stop one by id or every one on a buffer, stop all, and play
//! every voice into a period, freeing the ones that finish.
//!
//! Real-time safe: the array is inline and fixed, and each method is one
//! pass over at most `VOICE_COUNT` voices. Nothing here allocates, locks or
//! fails.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::config::VOICE_COUNT;
use crate::dsp::Playhead;
use crate::dsp::command::{BufferAddr, VoiceId};

/// One sound playing, and the id the engine named it by.
#[derive(Debug)]
struct Voice {
    id: VoiceId,
    playhead: Playhead,
}

/// Every voice there is; a free one holds nothing.
#[derive(Debug)]
pub struct Voices([Option<Voice>; VOICE_COUNT]);

impl Voices {
    /// No sounds playing.
    pub fn empty() -> Self {
        Self(std::array::from_fn(|_| None))
    }

    /// Starts `audio` from its first frame as `id`, in a free voice or,
    /// if there is none, in place of the oldest sound. Ids only go up, so
    /// the oldest is the lowest.
    pub fn start(&mut self, id: VoiceId, audio: Arc<AudioBuffer>) {
        let voice = match self.0.iter_mut().find(|s| s.is_none()) {
            Some(empty) => empty,
            None => self
                .0
                .iter_mut()
                .min_by_key(|s| s.as_ref().map(|voice| voice.id))
                .expect("VOICE_COUNT is above zero"),
        };
        *voice = Some(Voice {
            id,
            playhead: Playhead::new(audio),
        });
    }

    /// Stops the sound `id`, if it is still playing. A sound that has
    /// finished or been stolen is left gone.
    pub fn stop(&mut self, id: VoiceId) {
        for voice in self.0.iter_mut() {
            if voice.as_ref().is_some_and(|s| s.id == id) {
                *voice = None;
            }
        }
    }

    /// Stops every sound.
    pub fn stop_all(&mut self) {
        self.0.fill_with(|| None);
    }

    /// Stops every sound playing the buffer at `addr`, dropping their
    /// references to it. A buffer nothing plays costs the pass and
    /// changes nothing.
    pub fn evict(&mut self, addr: BufferAddr) {
        for voice in self.0.iter_mut() {
            if voice
                .as_ref()
                .is_some_and(|s| BufferAddr::of(s.playhead.audio()) == addr)
            {
                *voice = None;
            }
        }
    }

    /// Plays every voice's next period into `out`, summing them, and
    /// frees each voice whose sound has finished. Nothing is clipped
    /// here.
    pub fn play_all(&mut self, out: &mut [f32]) {
        for voice in self.0.iter_mut() {
            if let Some(s) = voice {
                s.playhead.play(out);
                if s.playhead.finished() {
                    *voice = None;
                }
            }
        }
    }

    /// Whether the sound `id` is playing. A front panel will want this;
    /// until then only tests ask.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: VoiceId) -> bool {
        self.0.iter().flatten().any(|s| s.id == id)
    }

    /// How many sounds are playing.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.0.iter().flatten().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Pcm;
    use crate::testing::audio;

    const HALF: Pcm = 0.5;

    fn mono(data: Vec<Pcm>) -> Arc<AudioBuffer> {
        Arc::new(audio(1, &data))
    }

    #[test]
    fn a_started_sound_is_heard_and_a_finished_one_leaves() {
        let mut voices = Voices::empty();
        voices.start(VoiceId(1), mono(vec![HALF; 3]));
        let mut out = vec![0.0; 4];
        voices.play_all(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(voices.is_playing(VoiceId(1)));

        let mut out = vec![0.0; 4];
        voices.play_all(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(!voices.is_playing(VoiceId(1)));
        assert_eq!(voices.playing(), 0);
    }

    #[test]
    fn sounds_sum() {
        let mut voices = Voices::empty();
        voices.start(VoiceId(1), mono(vec![HALF; 2]));
        voices.start(VoiceId(2), mono(vec![HALF; 2]));
        let mut out = vec![0.0; 4];
        voices.play_all(&mut out);
        assert_eq!(out, vec![1.0; 4]);
    }

    #[test]
    fn stop_silences_one_sound_and_stop_all_silences_every_sound() {
        let mut voices = Voices::empty();
        let long = mono(vec![HALF; 100]);
        for id in 1..=3 {
            voices.start(VoiceId(id), long.clone());
        }
        assert_eq!(voices.playing(), 3);

        voices.stop(VoiceId(2));
        assert!(!voices.is_playing(VoiceId(2)));
        assert_eq!(voices.playing(), 2);

        voices.stop_all();
        assert_eq!(voices.playing(), 0);
    }

    #[test]
    fn stopping_a_sound_that_is_gone_does_nothing() {
        let mut voices = Voices::empty();
        voices.start(VoiceId(1), mono(vec![HALF; 100]));
        voices.stop(VoiceId(99));
        assert_eq!(voices.playing(), 1);
    }

    #[test]
    fn a_press_with_every_voice_taken_steals_the_oldest_sound() {
        let mut voices = Voices::empty();
        let long = mono(vec![HALF; 100]);
        for id in 0..VOICE_COUNT as u64 {
            voices.start(VoiceId(id), long.clone());
        }
        assert_eq!(voices.playing(), VOICE_COUNT);

        voices.start(VoiceId(1000), long.clone());
        assert_eq!(voices.playing(), VOICE_COUNT);
        assert!(!voices.is_playing(VoiceId(0)));
        assert!(voices.is_playing(VoiceId(1)));
        assert!(voices.is_playing(VoiceId(1000)));

        // The next steal takes the next oldest, not the newcomer.
        voices.start(VoiceId(1001), long);
        assert!(!voices.is_playing(VoiceId(1)));
        assert!(voices.is_playing(VoiceId(1000)));
    }

    #[test]
    fn a_freed_voice_is_reused_before_anything_is_stolen() {
        let mut voices = Voices::empty();
        let long = mono(vec![HALF; 100]);
        for id in 0..VOICE_COUNT as u64 {
            voices.start(VoiceId(id), long.clone());
        }
        voices.stop(VoiceId(5));
        voices.start(VoiceId(1000), long);
        assert!(voices.is_playing(VoiceId(0)));
        assert!(voices.is_playing(VoiceId(1000)));
        assert_eq!(voices.playing(), VOICE_COUNT);
    }

    #[test]
    fn evict_stops_every_sound_on_that_buffer_and_no_other() {
        let mut voices = Voices::empty();
        let gone = mono(vec![HALF; 100]);
        let kept = mono(vec![HALF; 100]);
        voices.start(VoiceId(1), gone.clone());
        voices.start(VoiceId(2), kept.clone());
        voices.start(VoiceId(3), gone.clone());

        voices.evict(BufferAddr::of(&gone));
        assert!(!voices.is_playing(VoiceId(1)));
        assert!(voices.is_playing(VoiceId(2)));
        assert!(!voices.is_playing(VoiceId(3)));
        // The evicted sounds dropped their references at once.
        assert_eq!(Arc::strong_count(&gone), 1);
        assert_eq!(Arc::strong_count(&kept), 2);
    }

    #[test]
    fn evicting_a_buffer_nothing_plays_does_nothing() {
        let mut voices = Voices::empty();
        voices.start(VoiceId(1), mono(vec![HALF; 100]));
        let other = mono(vec![HALF; 2]);
        voices.evict(BufferAddr::of(&other));
        assert_eq!(voices.playing(), 1);
    }

    #[test]
    fn a_stopped_sound_lets_go_of_its_audio() {
        let mut voices = Voices::empty();
        let audio = mono(vec![HALF; 100]);
        voices.start(VoiceId(1), audio.clone());
        assert_eq!(Arc::strong_count(&audio), 2);
        voices.stop(VoiceId(1));
        assert_eq!(Arc::strong_count(&audio), 1);
    }
}
