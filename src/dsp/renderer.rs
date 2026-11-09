//! Commands in, a period of audio out.
//!
//! The [`Renderer`] is real-time safe, not real-time only. Each call to
//! `render` is given a stereo `f32` slice; the renderer applies the
//! commands that have arrived, then zeroes the slice, lets each source
//! add itself, clips the sum once, and returns. Nothing in here
//! allocates, locks, blocks, fails or reads a clock, so an audio device
//! can call it from its callback with a hard deadline, and a bounce can
//! call it in a loop as fast as the CPU goes and get the same frames.
//!
//! The renderer is the wiring: it says which sources there are and the
//! order they are summed, and maps each command onto the piece it is
//! for. What a source does to make its frames is that source's business;
//! today there is one, the [`VoiceTable`] playing.
//!
//! Commands come over a lock-free ring. The renderer owns the consuming
//! end; the engine owns the producing end. The engine mints one renderer
//! and hands it out as its output; everything else here is the engine's
//! business, so `render` is the one public method.

use rtrb::Consumer;

use crate::dsp::command::Command;
#[cfg(test)]
use crate::dsp::command::VoiceId;
use crate::dsp::voice_table::VoiceTable;

/// The render side of an engine: turns commands and resident audio into
/// periods of output.
///
/// Taken once from `Engine::output` and moved to whoever owns the audio
/// thread: into a device callback for live sound, or a loop for the same
/// frames offline.
///
/// **Drop whatever drives this before dropping the engine.** A voice still
/// ringing holds a reference to its audio; if the engine goes first, that
/// reference is the last one and the buffer is freed on the audio thread
/// when the voice ends.
#[derive(Debug)]
pub struct Renderer {
    /// The consuming end of the command ring.
    commands: Consumer<Command>,
    /// Every voice playing, summed as one source.
    voice_table: VoiceTable,
}

impl Renderer {
    /// A silent renderer that takes its commands from `commands`.
    pub(crate) fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            voice_table: VoiceTable::new(),
        }
    }

    /// Renders one period into `out`, an interleaved stereo `f32` slice,
    /// replacing whatever it held.
    ///
    /// Commands the engine has sent since the last call are applied first,
    /// so a play, stop or eviction lands at a period boundary. Then each
    /// source adds itself, and the sum is clipped once to the unit range.
    pub fn render(&mut self, out: &mut [f32]) {
        self.apply_commands();
        out.fill(0.0);
        self.voice_table.mix_into(out);
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
    }

    /// Applies every command that has arrived, each to the piece it is
    /// for.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Play { id, audio } => self.voice_table.play(id, audio),
                Command::Stop(id) => self.voice_table.stop(id),
                Command::StopAll => self.voice_table.stop_all(),
                Command::Evict(addr) => self.voice_table.evict(addr),
            }
        }
    }

    /// Whether voice `id` is playing.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: VoiceId) -> bool {
        self.voice_table.is_playing(id)
    }

    /// How many voices are playing.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.voice_table.playing()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::{Pcm, VOICE_COUNT};
    use crate::dsp::command::{BufferAddr, VoiceId};
    use crate::testing::audio;

    const HALF: Pcm = 0.5;
    const FULL: Pcm = 1.0;

    fn renderer() -> (Producer<Command>, Renderer) {
        let (producer, consumer) = RingBuffer::new(VOICE_COUNT + 8);
        (producer, Renderer::new(consumer))
    }

    fn mono(data: Vec<Pcm>) -> Arc<AudioBuffer> {
        Arc::new(audio(1, &data))
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
    fn evict_stops_every_voice_on_that_buffer_and_no_other() {
        let (mut p, mut r) = renderer();
        let gone = mono(vec![HALF; 100]);
        let kept = mono(vec![HALF; 100]);
        play(&mut p, 1, &gone);
        play(&mut p, 2, &kept);
        play(&mut p, 3, &gone);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 3);

        p.push(Command::Evict(BufferAddr::of(&gone))).unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(VoiceId(1)));
        assert!(r.is_playing(VoiceId(2)));
        assert!(!r.is_playing(VoiceId(3)));
        assert_eq!(out, vec![0.5; 4]);
        // The evicted voices dropped their references in the same period.
        assert_eq!(Arc::strong_count(&gone), 1);
        assert_eq!(Arc::strong_count(&kept), 2);
    }

    #[test]
    fn evicting_a_buffer_nothing_plays_does_nothing() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF; 100]));
        let other = mono(vec![HALF; 2]);
        p.push(Command::Evict(BufferAddr::of(&other))).unwrap();
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 1);
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
