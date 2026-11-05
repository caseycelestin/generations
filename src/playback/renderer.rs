//! The real-time side: commands in, a period of audio out.
//!
//! The [`Renderer`] lives on the audio thread. Each period the device
//! hands it a stereo `f32` slice; the renderer applies the commands that
//! have arrived, which start and stop voices, then zeroes the slice, lets
//! every playing voice add itself, clips the sum once, and returns.
//! Nothing in here may allocate, lock, block or fail.
//!
//! Commands come over a lock-free ring from the control thread. The
//! renderer owns the consuming end; the engine owns the producing end
//! and is the only other code that knows the renderer exists.
//!
use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::VOICE_COUNT;
use crate::playback::voice::Voice;

/// Identity of one started sound, minted by the engine.
///
/// Minted in order and never reused, so two things hold: a stop that
/// arrives after its voice was stolen or finished names nothing, and the
/// lowest id among playing voices is the oldest, which is what a full
/// table steals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VoiceId(pub u64);

/// What the engine can ask of the renderer.
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

/// The real-time side of playback: what the audio thread runs.
#[derive(Debug)]
pub struct Renderer {
    /// The consuming end of the command ring.
    commands: Consumer<Command>,
    /// The voices playing. Fixed size, so the work in one period is
    /// bounded; an empty slot is `None`.
    voices: [Option<Voice>; VOICE_COUNT],
}

impl Renderer {
    /// A silent renderer that takes its commands from `commands`.
    pub fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            voices: [const { None }; VOICE_COUNT],
        }
    }

    /// Renders one period into `out`, an interleaved stereo slice,
    /// replacing whatever it held.
    ///
    /// Commands that arrived since the last period are applied first, so a
    /// sound starts or stops at a period boundary. Then every voice adds
    /// itself, a voice that reaches its end frees its slot, and the sum is
    /// clipped once to the unit range.
    pub fn render(&mut self, out: &mut [f32]) {
        self.apply_commands();
        out.fill(0.0);
        for slot in &mut self.voices {
            if let Some(voice) = slot
                && voice.mix_into(out)
            {
                *slot = None;
            }
        }
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
    }

    /// Applies every command that has arrived.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Play { id, audio } => self.play(id, audio),
                Command::Stop(id) => self.stop(id),
                Command::StopAll => self.voices.fill_with(|| None),
            }
        }
    }

    /// Starts a voice in a free slot, or in the oldest voice's slot when
    /// none is free. Ids are minted in order, so the oldest is the lowest.
    fn play(&mut self, id: VoiceId, audio: Arc<AudioBuffer>) {
        // `None` orders below every `Some`, so a free slot wins when there
        // is one and the lowest id wins otherwise.
        let slot = self
            .voices
            .iter_mut()
            .min_by_key(|slot| slot.as_ref().map(Voice::id))
            .expect("VOICE_COUNT is not zero; see config");
        *slot = Some(Voice::new(id, audio));
    }

    /// Stops voice `id`. A voice that is gone is left gone.
    fn stop(&mut self, id: VoiceId) {
        for slot in &mut self.voices {
            if slot.as_ref().is_some_and(|v| v.id() == id) {
                *slot = None;
            }
        }
    }

    /// Whether voice `id` is playing.
    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.voices
            .iter()
            .any(|v| v.as_ref().is_some_and(|v| v.id() == id))
    }

    /// How many voices are playing.
    pub fn playing(&self) -> usize {
        self.voices.iter().filter(|v| v.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::config::Pcm;

    const HALF: Pcm = 16_384;
    const FULL: Pcm = Pcm::MAX;

    fn renderer() -> (Producer<Command>, Renderer) {
        let (producer, consumer) = RingBuffer::new(VOICE_COUNT + 8);
        (producer, Renderer::new(consumer))
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
        let (_p, mut r) = renderer();
        let mut out = vec![0.7; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn a_played_voice_is_heard_from_the_next_render() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF, HALF]));
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.5; 4]);
    }

    #[test]
    fn voices_sum_and_the_sum_is_clipped() {
        let (mut p, mut r) = renderer();
        let loud = mono(vec![FULL, -FULL]);
        play(&mut p, 1, &loud);
        play(&mut p, 2, &loud);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![1.0, 1.0, -1.0, -1.0]);
    }

    #[test]
    fn a_finished_voice_frees_its_slot() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF; 3]));
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert!(r.is_playing(VoiceId(1)));
        r.render(&mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0, 0.0]);
        assert!(!r.is_playing(VoiceId(1)));
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn stop_silences_one_voice_and_stop_all_silences_every_voice() {
        let (mut p, mut r) = renderer();
        let long = mono(vec![HALF; 100]);
        play(&mut p, 1, &long);
        play(&mut p, 2, &long);
        play(&mut p, 3, &long);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 3);

        p.push(Command::Stop(VoiceId(2))).unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(VoiceId(2)));
        assert_eq!(r.playing(), 2);
        assert_eq!(out, vec![1.0; 4]);

        p.push(Command::StopAll).unwrap();
        r.render(&mut out);
        assert_eq!(r.playing(), 0);
        assert_eq!(out, vec![0.0; 4]);
    }

    #[test]
    fn stopping_a_voice_that_is_gone_does_nothing() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF; 100]));
        p.push(Command::Stop(VoiceId(99))).unwrap();
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 1);
    }

    #[test]
    fn a_full_table_steals_the_oldest_voice() {
        let (mut p, mut r) = renderer();
        let long = mono(vec![HALF; 100]);
        for id in 0..VOICE_COUNT as u64 {
            play(&mut p, id, &long);
        }
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), VOICE_COUNT);

        play(&mut p, 1000, &long);
        r.render(&mut out);
        assert_eq!(r.playing(), VOICE_COUNT);
        assert!(!r.is_playing(VoiceId(0)));
        assert!(r.is_playing(VoiceId(1)));
        assert!(r.is_playing(VoiceId(1000)));

        // The next steal takes the next oldest, not the newcomer.
        play(&mut p, 1001, &long);
        r.render(&mut out);
        assert!(!r.is_playing(VoiceId(1)));
        assert!(r.is_playing(VoiceId(1000)));
    }

    #[test]
    fn a_stopped_voice_lets_go_of_its_audio() {
        let (mut p, mut r) = renderer();
        let audio = mono(vec![HALF; 100]);
        play(&mut p, 1, &audio);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(Arc::strong_count(&audio), 2);
        p.push(Command::Stop(VoiceId(1))).unwrap();
        r.render(&mut out);
        assert_eq!(Arc::strong_count(&audio), 1);
    }
}
