//! Commands in, a period of audio out.
//!
//! The [`Renderer`] is real-time safe, not real-time only. Each call to
//! `render` is given a stereo `f32` slice; the renderer applies the
//! commands that have arrived, then zeroes the slice, runs the path over
//! it, and returns. Nothing in here allocates, locks, blocks, fails or
//! reads a clock, so an audio device can call it from its callback with
//! a hard deadline, and a bounce can call it in a loop as fast as the
//! CPU goes and get the same frames.
//!
//! The renderer is the wiring: it owns every piece of state the path
//! needs, says which functions run over it and in what order, and maps
//! each command onto the state it changes. See the `dsp` module doc for
//! the pieces and the convention they share. Today the path is: every
//! voice plays into the period, then a hard clip.
//!
//! Commands come over a lock-free ring. The renderer owns the consuming
//! end; the engine owns the producing end. The engine mints one renderer
//! and hands it out as its output; everything else here is the engine's
//! business, so `render` is the one public method.

use rtrb::Consumer;

use crate::dsp::command::Command;
#[cfg(test)]
use crate::dsp::command::VoiceId;
use crate::dsp::{Voices, hard_clip};

/// The render side of an engine: turns commands and resident audio into
/// periods of output.
///
/// Taken once from `Engine::output` and moved to whoever owns the audio
/// thread: into a device callback for live sound, or a loop for the same
/// frames offline.
///
/// **Drop whatever drives this before dropping the engine.** A sound
/// still playing holds a reference to its audio; if the engine goes
/// first, that reference is the last one and the buffer is freed on the
/// audio thread when the sound ends.
#[derive(Debug)]
pub struct Renderer {
    /// The consuming end of the command ring.
    commands: Consumer<Command>,
    /// The sounds playing.
    voices: Voices,
}

impl Renderer {
    /// A silent renderer that takes its commands from `commands`.
    pub(crate) fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            voices: Voices::empty(),
        }
    }

    /// Renders one period into `out`, an interleaved stereo `f32` slice,
    /// replacing whatever it held.
    ///
    /// Commands the engine has sent since the last call are applied first,
    /// so a play, stop or eviction lands at a period boundary. Then every
    /// voice plays into the zeroed slice, a sound that reaches its end
    /// frees its voice, and the sum is clipped once to the unit range.
    pub fn render(&mut self, out: &mut [f32]) {
        self.apply_commands();
        out.fill(0.0);
        self.voices.play_all(out);
        hard_clip(out);
    }

    /// Applies every command that has arrived, each to the state it is
    /// for.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Play { id, audio } => self.voices.start(id, audio),
                Command::Stop(id) => self.voices.stop(id),
                Command::StopAll => self.voices.stop_all(),
                Command::Evict(addr) => self.voices.evict(addr),
            }
        }
    }

    /// Whether the sound `id` is playing.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: VoiceId) -> bool {
        self.voices.is_playing(id)
    }

    /// How many sounds are playing.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.voices.playing()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::Pcm;
    use crate::dsp::command::BufferAddr;
    use crate::testing::audio;

    const HALF: Pcm = 0.5;
    const FULL: Pcm = 1.0;

    fn renderer() -> (Producer<Command>, Renderer) {
        let (producer, consumer) = RingBuffer::new(8);
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
    fn a_play_is_heard_from_the_next_render() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF, HALF]));
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.5; 4]);
    }

    #[test]
    fn sounds_sum_and_the_sum_is_clipped() {
        let (mut p, mut r) = renderer();
        let loud = mono(vec![FULL, -FULL]);
        play(&mut p, 1, &loud);
        play(&mut p, 2, &loud);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![1.0, 1.0, -1.0, -1.0]);
    }

    #[test]
    fn stop_and_evict_land_at_the_next_render() {
        let (mut p, mut r) = renderer();
        let a = mono(vec![HALF; 100]);
        let b = mono(vec![HALF; 100]);
        play(&mut p, 1, &a);
        play(&mut p, 2, &b);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 2);

        p.push(Command::Stop(VoiceId(1))).unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(VoiceId(1)));
        assert_eq!(out, vec![0.5; 4]);

        p.push(Command::Evict(BufferAddr::of(&b))).unwrap();
        r.render(&mut out);
        assert_eq!(r.playing(), 0);
        assert_eq!(out, vec![0.0; 4]);
    }
}
