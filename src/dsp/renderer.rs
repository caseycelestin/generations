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
//! each command onto the mixer it is for. See the `dsp` module doc for
//! the pieces and the convention they share. Today the path is: the
//! samples heard add into the period, then the inputs heard, then a hard
//! clip.
//!
//! Commands come over a lock-free ring. The renderer owns the consuming
//! end; the engine owns the producing end. The engine mints one renderer
//! and hands it out as its output; everything else here is the engine's
//! business, so `render` is the one public method.

use rtrb::Consumer;

use crate::config::{INPUT_COUNT, SAMPLE_PLAYBACK_COUNT};
use crate::dsp::command::Command;
#[cfg(test)]
use crate::dsp::command::{InputCommand, SampleCommand, SoundId};
use crate::dsp::math::hard_clip;
use crate::dsp::{InputReader, Mixer, Playhead};

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
    samples: Mixer<Playhead, SAMPLE_PLAYBACK_COUNT>,
    /// The inputs heard.
    inputs: Mixer<InputReader, INPUT_COUNT>,
}

impl Renderer {
    /// A silent renderer that takes its commands from `commands`.
    pub(crate) fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            samples: Mixer::empty(),
            inputs: Mixer::empty(),
        }
    }

    /// Renders one period into `out`, an interleaved stereo `f32` slice,
    /// replacing whatever it held.
    ///
    /// Commands the engine has sent since the last call are applied first,
    /// so a play, stop or eviction lands at a period boundary. Then every
    /// sample and every input heard adds into the zeroed slice, one that
    /// reaches its end frees its slot, and the sum is clipped once to the
    /// unit range.
    pub fn render(&mut self, out: &mut [f32]) {
        self.apply_commands();
        out.fill(0.0);
        self.samples.add(out);
        self.inputs.add(out);
        hard_clip(out);
    }

    /// Applies every command that has arrived, each by the mixer it is
    /// for.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Sample(command) => self.samples.apply(command),
                Command::Input(command) => self.inputs.apply(command),
            }
        }
    }

    /// Whether the sound or input `id` is heard.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: SoundId) -> bool {
        self.samples.is_playing(id) || self.inputs.is_playing(id)
    }

    /// How many sounds and inputs are heard.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.samples.playing() + self.inputs.playing()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::{INPUT_CHANNELS, Pcm};
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
        p.push(
            SampleCommand::Play {
                id: SoundId(id),
                audio: audio.clone(),
            }
            .into(),
        )
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

    fn feed_two(writer: &mut Producer<Pcm>) {
        for _ in 0..2 {
            writer.push(0.25).unwrap();
            writer.push(-0.25).unwrap();
        }
    }

    #[test]
    fn an_input_is_silent_until_monitored_and_removed_by_its_own_command() {
        let (mut p, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF; 100]));
        let (mut writer, reader) = RingBuffer::new(8 * INPUT_CHANNELS);
        p.push(
            InputCommand::Add {
                id: SoundId(2),
                reader,
                sides: [0, 1],
            }
            .into(),
        )
        .unwrap();
        feed_two(&mut writer);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.5; 4], "an input is not heard until monitored");
        assert_eq!(r.playing(), 2);
        assert_eq!(
            writer.slots(),
            8 * INPUT_CHANNELS,
            "but it keeps up with its ring"
        );

        p.push(InputCommand::Monitor(SoundId(2)).into()).unwrap();
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.75, 0.25, 0.75, 0.25]);

        // A sample stop and a stop-all leave the input alone.
        p.push(SampleCommand::Stop(SoundId(2)).into()).unwrap();
        p.push(SampleCommand::StopAll.into()).unwrap();
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.25, -0.25, 0.25, -0.25]);

        p.push(InputCommand::Unmonitor(SoundId(2)).into()).unwrap();
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.0; 4]);
        assert!(r.is_playing(SoundId(2)), "unmonitored, still an input");

        p.push(InputCommand::Remove(SoundId(2)).into()).unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(SoundId(2)));
        // The reader is gone, so the input's writer feeds nothing.
        assert!(writer.is_abandoned());
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

        p.push(SampleCommand::Stop(SoundId(1)).into()).unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(SoundId(1)));
        assert_eq!(out, vec![0.5; 4]);

        p.push(SampleCommand::Evict(BufferAddr::of(&b)).into())
            .unwrap();
        r.render(&mut out);
        assert_eq!(r.playing(), 0);
        assert_eq!(out, vec![0.0; 4]);
    }
}
