//! A fixed set of sources summed into the period: the mixer.
//!
//! A [`Source`] is anything that adds its next period into the output
//! and knows when it is done: a [`Playhead`] over a loaded sample, an
//! [`InputReader`](crate::dsp::InputReader) over the capture ring. A
//! [`Mixer`] is `N` slots for one kind of source, each the sound the
//! engine named by a [`SoundId`]. Its methods are the whole policy:
//! start a source, stop one by id, stop all, mute and unmute one, and
//! add every slot into a period, freeing the ones that finish. As many
//! sources can be heard at once as there are slots, and a start that
//! finds them all taken steals the oldest. A muted slot runs on in
//! silence: its source skips the period instead of adding it, so a muted
//! sample still ends and a muted input still keeps up with its ring.
//!
//! The renderer owns one mixer per kind of source, so a table of
//! samples never steals from the table of inputs or the reverse.
//!
//! Real-time safe: the array is inline and fixed, and each method is one
//! pass over at most `N` slots. Nothing here allocates, locks or fails.

use crate::config::{OUTPUT_CHANNELS, Pcm};
use crate::dsp::Playhead;
use crate::dsp::command::{BufferAddr, SoundId};

/// Something a mixer slot can hold: it adds its next period into the
/// output, and says when it never will again.
pub trait Source {
    /// Adds the next period into `out`, an interleaved stereo slice.
    /// Nothing is clipped here.
    fn add(&mut self, out: &mut [Pcm]);
    /// Moves past the next `frames` frames without adding them: what a
    /// muted slot does instead of [`add`](Self::add).
    fn skip(&mut self, frames: usize);
    /// Whether [`add`](Self::add) will never change frames again, so the
    /// slot can be freed.
    fn finished(&self) -> bool;
}

/// One source in the mixer, and the id the engine named it by.
#[derive(Debug)]
struct Slot<S> {
    id: SoundId,
    source: S,
    /// Running but not heard.
    muted: bool,
}

/// `N` slots for sources of one kind; a free one holds nothing.
#[derive(Debug)]
pub struct Mixer<S, const N: usize>([Option<Slot<S>>; N]);

impl<S: Source, const N: usize> Mixer<S, N> {
    /// Nothing heard.
    pub fn empty() -> Self {
        const { assert!(N > 0, "a mixer needs a slot") };
        Self(std::array::from_fn(|_| None))
    }

    /// Starts `source` as `id`, heard, in a free slot or, if there is
    /// none, in place of the oldest. Ids only go up, so the oldest is the
    /// lowest.
    pub fn start(&mut self, id: SoundId, source: S) {
        let slot = match self.0.iter_mut().find(|s| s.is_none()) {
            Some(empty) => empty,
            None => self
                .0
                .iter_mut()
                .min_by_key(|s| s.as_ref().map(|slot| slot.id))
                .expect("N is above zero"),
        };
        *slot = Some(Slot {
            id,
            source,
            muted: false,
        });
    }

    /// Mutes or unmutes `id`, if it is in the mixer. A muted source runs
    /// on in silence and is heard again from the period after it is
    /// unmuted.
    pub fn set_muted(&mut self, id: SoundId, muted: bool) {
        for slot in self.0.iter_mut().flatten() {
            if slot.id == id {
                slot.muted = muted;
            }
        }
    }

    /// Stops `id`, if it is still heard. One that has finished or been
    /// stolen is left gone.
    pub fn stop(&mut self, id: SoundId) {
        for slot in self.0.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.id == id) {
                *slot = None;
            }
        }
    }

    /// Stops every source.
    pub fn stop_all(&mut self) {
        self.0.fill_with(|| None);
    }

    /// Adds every unmuted source's next period into `out`, summing them,
    /// moves every muted one past it, and frees each slot whose source has
    /// finished. Nothing is clipped here.
    pub fn add(&mut self, out: &mut [Pcm]) {
        let frames = out.len() / OUTPUT_CHANNELS;
        for slot in self.0.iter_mut() {
            if let Some(s) = slot {
                if s.muted {
                    s.source.skip(frames);
                } else {
                    s.source.add(out);
                }
                if s.source.finished() {
                    *slot = None;
                }
            }
        }
    }

    /// Whether `id` is in the mixer, muted or not. A front panel will
    /// want this; until then only tests ask.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: SoundId) -> bool {
        self.0.iter().flatten().any(|s| s.id == id)
    }

    /// Whether `id` is in the mixer and muted.
    #[cfg(test)]
    pub(crate) fn is_muted(&self, id: SoundId) -> bool {
        self.0.iter().flatten().any(|s| s.id == id && s.muted)
    }

    /// How many sources are heard.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.0.iter().flatten().count()
    }
}

impl<const N: usize> Mixer<Playhead, N> {
    /// Stops every sound playing the buffer at `addr`, dropping their
    /// references to it. A buffer nothing plays costs the pass and
    /// changes nothing. Only a sample has a buffer, so only this mixer
    /// can be asked.
    pub fn evict(&mut self, addr: BufferAddr) {
        for slot in self.0.iter_mut() {
            if slot
                .as_ref()
                .is_some_and(|s| BufferAddr::of(s.source.audio()) == addr)
            {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::testing::audio;

    const HALF: Pcm = 0.5;
    const SLOTS: usize = 4;

    type Samples = Mixer<Playhead, SLOTS>;

    fn mono(data: Vec<Pcm>) -> Arc<AudioBuffer> {
        Arc::new(audio(1, &data))
    }

    fn start(m: &mut Samples, id: u64, audio: &Arc<AudioBuffer>) {
        m.start(SoundId(id), Playhead::new(audio.clone()));
    }

    #[test]
    fn a_started_sound_is_heard_and_a_finished_one_leaves() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 3]));
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(m.is_playing(SoundId(1)));

        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(!m.is_playing(SoundId(1)));
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn sources_sum() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 2]));
        start(&mut m, 2, &mono(vec![HALF; 2]));
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![1.0; 4]);
    }

    #[test]
    fn stop_silences_one_and_stop_all_silences_every_one() {
        let mut m = Samples::empty();
        let long = mono(vec![HALF; 100]);
        for id in 1..=3 {
            start(&mut m, id, &long);
        }
        assert_eq!(m.playing(), 3);

        m.stop(SoundId(2));
        assert!(!m.is_playing(SoundId(2)));
        assert_eq!(m.playing(), 2);

        m.stop_all();
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn stopping_one_that_is_gone_does_nothing() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 100]));
        m.stop(SoundId(99));
        assert_eq!(m.playing(), 1);
    }

    #[test]
    fn a_start_with_every_slot_taken_steals_the_oldest() {
        let mut m = Samples::empty();
        let long = mono(vec![HALF; 100]);
        for id in 0..SLOTS as u64 {
            start(&mut m, id, &long);
        }
        assert_eq!(m.playing(), SLOTS);

        start(&mut m, 1000, &long);
        assert_eq!(m.playing(), SLOTS);
        assert!(!m.is_playing(SoundId(0)));
        assert!(m.is_playing(SoundId(1)));
        assert!(m.is_playing(SoundId(1000)));

        // The next steal takes the next oldest, not the newcomer.
        start(&mut m, 1001, &long);
        assert!(!m.is_playing(SoundId(1)));
        assert!(m.is_playing(SoundId(1000)));
    }

    #[test]
    fn a_freed_slot_is_reused_before_anything_is_stolen() {
        let mut m = Samples::empty();
        let long = mono(vec![HALF; 100]);
        for id in 0..SLOTS as u64 {
            start(&mut m, id, &long);
        }
        m.stop(SoundId(2));
        start(&mut m, 1000, &long);
        assert!(m.is_playing(SoundId(0)));
        assert!(m.is_playing(SoundId(1000)));
        assert_eq!(m.playing(), SLOTS);
    }

    #[test]
    fn a_muted_source_runs_on_in_silence_and_is_heard_when_unmuted() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 6]));
        m.set_muted(SoundId(1), true);
        assert!(m.is_muted(SoundId(1)));
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![0.0; 4]);

        // Two of six frames went by unheard; the rest are heard.
        m.set_muted(SoundId(1), false);
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(out, vec![0.5; 4]);
        assert!(!m.is_playing(SoundId(1)));
    }

    #[test]
    fn a_muted_source_still_finishes() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 2]));
        m.set_muted(SoundId(1), true);
        let mut out = vec![0.0; 4];
        m.add(&mut out);
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn muting_one_that_is_gone_does_nothing() {
        let mut m = Samples::empty();
        m.set_muted(SoundId(7), true);
        assert!(!m.is_muted(SoundId(7)));
    }

    #[test]
    fn evict_stops_every_sound_on_that_buffer_and_no_other() {
        let mut m = Samples::empty();
        let gone = mono(vec![HALF; 100]);
        let kept = mono(vec![HALF; 100]);
        start(&mut m, 1, &gone);
        start(&mut m, 2, &kept);
        start(&mut m, 3, &gone);

        m.evict(BufferAddr::of(&gone));
        assert!(!m.is_playing(SoundId(1)));
        assert!(m.is_playing(SoundId(2)));
        assert!(!m.is_playing(SoundId(3)));
        // The evicted sounds dropped their references at once.
        assert_eq!(Arc::strong_count(&gone), 1);
        assert_eq!(Arc::strong_count(&kept), 2);
    }

    #[test]
    fn evicting_a_buffer_nothing_plays_does_nothing() {
        let mut m = Samples::empty();
        start(&mut m, 1, &mono(vec![HALF; 100]));
        let other = mono(vec![HALF; 2]);
        m.evict(BufferAddr::of(&other));
        assert_eq!(m.playing(), 1);
    }

    #[test]
    fn a_stopped_sound_lets_go_of_its_audio() {
        let mut m = Samples::empty();
        let audio = mono(vec![HALF; 100]);
        start(&mut m, 1, &audio);
        assert_eq!(Arc::strong_count(&audio), 2);
        m.stop(SoundId(1));
        assert_eq!(Arc::strong_count(&audio), 1);
    }
}
