//! The voices summed into a period.
//!
//! The [`Mixer`] lives on the real-time thread. Each period the device
//! hands it a stereo `f32` slice and it does five things: takes the
//! commands that arrived since last time, zeroes the slice, lets every
//! playing voice add itself, clips the sum once, and returns. Nothing in
//! here allocates, locks, blocks or fails.
//!
//! Commands come over a lock-free ring from the control thread. The mixer
//! owns the consuming end; whoever owns the producing end is the only
//! other code that knows the mixer exists.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::VOICE_COUNT;
use crate::playback::voice::Voice;

/// Identity of one started sound, minted on the control thread.
///
/// Never reused, so a stop that arrives after its voice was stolen or
/// finished names nothing and does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u64);

/// What the control thread can ask of the mixer.
#[derive(Debug)]
pub enum Command {
    /// Start `audio` from its first frame as voice `id`.
    Play {
        id: VoiceId,
        audio: Arc<AudioBuffer>,
    },
    /// Stop voice `id` if it is still playing.
    Stop(VoiceId),
    /// Stop every voice.
    StopAll,
}

/// One occupied slot in the voice table.
#[derive(Debug)]
struct Slot {
    id: VoiceId,
    /// When this voice started, in starts: the lowest is the oldest.
    started: u64,
    voice: Voice,
}

/// The voices summed into a period.
#[derive(Debug)]
pub struct Mixer {
    commands: Consumer<Command>,
    slots: [Option<Slot>; VOICE_COUNT],
    /// How many voices have ever started; stamps each new slot.
    starts: u64,
}

impl Mixer {
    /// A silent mixer that takes its commands from `commands`.
    pub fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            slots: [const { None }; VOICE_COUNT],
            starts: 0,
        }
    }

    /// Renders one period into `out`, an interleaved stereo slice, replacing
    /// whatever it held.
    pub fn render(&mut self, out: &mut [f32]) {
        self.take_commands();
        out.fill(0.0);
        for slot in &mut self.slots {
            if let Some(s) = slot
                && s.voice.mix_into(out)
            {
                *slot = None;
            }
        }
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
    }

    /// Applies every command that has arrived.
    fn take_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Play { id, audio } => self.start(id, audio),
                Command::Stop(id) => {
                    for slot in &mut self.slots {
                        if slot.as_ref().is_some_and(|s| s.id == id) {
                            *slot = None;
                        }
                    }
                }
                Command::StopAll => self.slots.fill_with(|| None),
            }
        }
    }

    /// Puts a new voice in a free slot, or in the oldest voice's slot when
    /// none is free.
    fn start(&mut self, id: VoiceId, audio: Arc<AudioBuffer>) {
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(free) => free,
            None => self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.as_ref().map_or(u64::MAX, |s| s.started))
                .map(|(i, _)| i)
                .expect("VOICE_COUNT is not zero"),
        };
        self.slots[slot] = Some(Slot {
            id,
            started: self.starts,
            voice: Voice::new(audio),
        });
        self.starts += 1;
    }

    /// Whether voice `id` is playing.
    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.slots
            .iter()
            .any(|s| s.as_ref().is_some_and(|s| s.id == id))
    }

    /// How many voices are playing.
    pub fn playing(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::config::Pcm;

    const HALF: Pcm = 16_384;
    const FULL: Pcm = Pcm::MAX;

    fn mixer() -> (Producer<Command>, Mixer) {
        let (producer, consumer) = RingBuffer::new(VOICE_COUNT + 8);
        (producer, Mixer::new(consumer))
    }

    fn mono(data: Vec<Pcm>) -> Arc<AudioBuffer> {
        Arc::new(AudioBuffer::new(1, data).unwrap())
    }

    fn play(p: &mut Producer<Command>, id: u64, audio: &Arc<AudioBuffer>) {
        p.push(Command::Play {
            id: VoiceId(id),
            audio: audio.clone(),
        })
        .unwrap();
    }

    #[test]
    fn silence_when_nothing_plays() {
        let (_p, mut m) = mixer();
        let mut out = vec![0.7; 4];
        m.render(&mut out);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(m.playing(), 0);
    }

    #[test]
    fn a_played_voice_is_heard_from_the_next_render() {
        let (mut p, mut m) = mixer();
        play(&mut p, 1, &mono(vec![HALF, HALF]));
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(out, vec![0.5; 4]);
    }

    #[test]
    fn voices_sum_and_the_sum_is_clipped() {
        let (mut p, mut m) = mixer();
        let loud = mono(vec![FULL, -FULL]);
        play(&mut p, 1, &loud);
        play(&mut p, 2, &loud);
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(out, vec![1.0, 1.0, -1.0, -1.0]);
    }

    #[test]
    fn a_finished_voice_frees_its_slot() {
        let (mut p, mut m) = mixer();
        play(&mut p, 1, &mono(vec![HALF; 3]));
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(m.playing(), 1);
        assert!(m.is_playing(VoiceId(1)));
        m.render(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert_eq!(m.playing(), 0);
        assert!(!m.is_playing(VoiceId(1)));
    }

    #[test]
    fn stop_silences_one_voice_and_stop_all_silences_every_voice() {
        let (mut p, mut m) = mixer();
        let long = mono(vec![HALF; 100]);
        play(&mut p, 1, &long);
        play(&mut p, 2, &long);
        play(&mut p, 3, &long);
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(m.playing(), 3);

        p.push(Command::Stop(VoiceId(2))).unwrap();
        m.render(&mut out);
        assert_eq!(m.playing(), 2);
        assert!(!m.is_playing(VoiceId(2)));
        assert_eq!(out, vec![1.0; 4]);

        p.push(Command::StopAll).unwrap();
        m.render(&mut out);
        assert_eq!(m.playing(), 0);
        assert_eq!(out, vec![0.0; 4]);
    }

    #[test]
    fn stopping_a_voice_that_is_gone_does_nothing() {
        let (mut p, mut m) = mixer();
        play(&mut p, 1, &mono(vec![HALF; 100]));
        p.push(Command::Stop(VoiceId(99))).unwrap();
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(m.playing(), 1);
    }

    #[test]
    fn a_full_table_steals_the_oldest_voice() {
        let (mut p, mut m) = mixer();
        let long = mono(vec![HALF; 100]);
        for id in 0..VOICE_COUNT as u64 {
            play(&mut p, id, &long);
        }
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(m.playing(), VOICE_COUNT);

        play(&mut p, 1000, &long);
        m.render(&mut out);
        assert_eq!(m.playing(), VOICE_COUNT);
        assert!(!m.is_playing(VoiceId(0)));
        assert!(m.is_playing(VoiceId(1)));
        assert!(m.is_playing(VoiceId(1000)));

        // The next steal takes the next oldest, not the newcomer.
        play(&mut p, 1001, &long);
        m.render(&mut out);
        assert!(!m.is_playing(VoiceId(1)));
        assert!(m.is_playing(VoiceId(1000)));
    }

    #[test]
    fn a_dropped_voice_lets_go_of_its_audio() {
        let (mut p, mut m) = mixer();
        let audio = mono(vec![HALF; 100]);
        play(&mut p, 1, &audio);
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert_eq!(Arc::strong_count(&audio), 2);
        p.push(Command::Stop(VoiceId(1))).unwrap();
        m.render(&mut out);
        assert_eq!(Arc::strong_count(&audio), 1);
    }
}
