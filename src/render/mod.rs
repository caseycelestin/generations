//! This crate's pipeline: commands and resident audio in, a period of
//! output out.
//!
//! ```text
//! control side                          render side
//! (Control)     ── Command ring ──▶     Renderer       the wiring: commands in, a period out
//!               ◀── Released ring ──      every sound let go of, to be dropped there
//!                                         Sounds       the places sounds are held, samples and inputs apart
//!                                           Sound      a source and its gain
//!                                             SampleReader  a loaded sample, first frame to last
//!                                             InputReader   one input sound's ring, a period at a time
//!               ── a ring per input sound ─▶              fed from the capture callback
//!                                         math         add, hard_clip: the period summed and clipped
//! ```
//!
//! The pieces come from [`dsp`](crate::render::dsp) and are generic; this
//! module is the one place that picks which of them this crate runs, in what
//! order, and what the control side may ask of them. It defines the
//! [`Command`]s it accepts and its handle builds them. Nothing here
//! knows what a project, a device or resident audio is: audio arrives
//! inside a command as an `Arc<AudioBuffer>`, and a sample reader holds its
//! clone until it finishes or is told to let go; each sound of an input
//! arrives as the reading end of its own ring the same way. The control
//! side names a sound to this side by its [`SoundId`](command::SoundId)
//! and a buffer by its [`BufferAddress`], never by what it is.
//!
//! The [`Renderer`] is real-time safe, not real-time only. Each call to
//! `render` is given a stereo slice; the renderer applies the commands
//! that have arrived, then zeroes the slice, adds every sound into it,
//! clips it, and returns. Nothing in here allocates, locks, blocks, fails or
//! reads a clock, so an audio device can call it from its callback with
//! a hard deadline, and a bounce can call it in a loop as fast as the
//! CPU goes and get the same frames.
//!
//! The renderer is the wiring: it owns every piece of state the path
//! needs, says what runs over it and in what order, and applies each
//! command to the sounds it is for. What to do when the places are full
//! is decided here too: a sample cuts the oldest, an input is refused.
//! See the `dsp` module doc for the pieces. Today the path is: every
//! sample sound adds into the period, then every input sound, then a
//! hard clip.
//!
//! Commands come over a lock-free ring. The renderer owns the consuming
//! end; its [`Control`] owns the producing end, and the two are
//! made together by [`Renderer::new`]. Outside the crate the render side
//! is one type with one method, `render`.
//!
//! Nothing is dropped here either. Every sound the renderer lets go of,
//! finished, removed, stolen, evicted or refused, goes back over a second
//! ring as a [`Released`], and the control drops it the next time it sends.
//! A sound can hold the last reference to a sample's audio, and freeing
//! that is a trip into the allocator and the kernel with no bound on how
//! long it takes. The ring has a place for every sound held and every
//! command in flight, so it cannot fill between two sends.

pub mod command;
pub(crate) mod dsp;
pub(crate) mod sound;
pub(crate) mod sounds;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::config::{MAX_PERIOD_FRAMES, Pcm};
use crate::control::Control;
use crate::render::dsp::math::{add, hard_clip};
use crate::render::dsp::{Gain, InputReader, SampleReader, Source};
#[cfg(test)]
use command::SoundId;
use command::{BufferAddress, Command, Released, SoundCommand};
use sound::Sound;
use sounds::Sounds;

/// How a renderer is sized, chosen per device: a laptop, a tablet and a
/// CM4 each want their own period and polyphony. The period is the one
/// thing every device must say, so [`RendererSettings::new`] takes it and
/// defaults the rest; a caller names only what else it changes:
///
/// ```
/// # use generations::{Renderer, RendererSettings};
/// let (renderer, control) = Renderer::new(RendererSettings {
///     sample_sounds: 24,
///     ..RendererSettings::new(256)
/// });
/// ```
///
/// Each count is bounded by the matching `Renderer::MAX_*`, so any
/// settings stay inside a worst case that has been thought about. All of
/// it is allocated once, in [`Renderer::new`], on the control side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererSettings {
    /// The most frames one call to [`Renderer::render`] is given: the
    /// output device's period, or the largest piece a caller renders at
    /// once. 1 to `MAX_PERIOD_FRAMES`, and no default: it is a fact of the
    /// device, and what the render side sizes its buffers from.
    pub period: usize,
    /// How many commands the handle can send before the renderer has had
    /// a period to take them. 1 to [`Renderer::MAX_COMMAND_CAPACITY`],
    /// which is the default.
    pub command_capacity: usize,
    /// Sample sounds held at once. When every place is taken, a play
    /// cuts the oldest. 1 to
    /// [`Renderer::MAX_SAMPLE_SOUNDS`], which is the default.
    pub sample_sounds: usize,
    /// Input sounds held at once across every input, one per mono or
    /// stereo entry of its layout. An input that would not fit is
    /// refused. 1 to [`Renderer::MAX_INPUT_SOUNDS`], which is the default.
    pub input_sounds: usize,
}

impl RendererSettings {
    /// Settings for renders of up to `period` frames, with every other
    /// field at its maximum.
    pub const fn new(period: usize) -> Self {
        Self {
            period,
            command_capacity: Renderer::MAX_COMMAND_CAPACITY,
            sample_sounds: Renderer::MAX_SAMPLE_SOUNDS,
            input_sounds: Renderer::MAX_INPUT_SOUNDS,
        }
    }

    /// Checks each field against its bound. A debug check: settings are
    /// written by the caller for a device, not taken from a user.
    fn debug_check(&self) {
        debug_assert!(
            (1..=MAX_PERIOD_FRAMES).contains(&self.period),
            "an output period is 1 to MAX_PERIOD_FRAMES frames"
        );
        debug_assert!(
            (1..=Renderer::MAX_COMMAND_CAPACITY).contains(&self.command_capacity),
            "a command capacity is 1 to MAX_COMMAND_CAPACITY"
        );
        debug_assert!(
            (1..=Renderer::MAX_SAMPLE_SOUNDS).contains(&self.sample_sounds),
            "sample sounds are 1 to MAX_SAMPLE_SOUNDS"
        );
        debug_assert!(
            (1..=Renderer::MAX_INPUT_SOUNDS).contains(&self.input_sounds),
            "input sounds are 1 to MAX_INPUT_SOUNDS"
        );
    }
}

/// The render side: turns commands and resident audio into periods of
/// output.
///
/// Made by [`Renderer::new`] together with its [`Control`], and
/// moved to whoever owns the audio thread: into a device callback for
/// live sound, or a loop for the same frames offline.
///
/// A sound it lets go of goes back to its `Control` to be dropped, so a
/// sample's audio is never freed on the audio thread, whichever goes
/// first. Dropping the renderer itself drops the sounds it still holds,
/// wherever that happens.
#[derive(Debug)]
pub struct Renderer {
    /// The consuming end of the command ring.
    commands: Consumer<Command>,
    /// The producing end of the ring every sound let go of goes back on.
    released: Producer<Released>,
    /// The most frames one call to `render` is given: the output period.
    period: usize,
    /// The samples playing, up to the settings' `sample_sounds`.
    samples: Sounds<Sound<SampleReader>>,
    /// The input sounds, one per mono or stereo entry of every input, up
    /// to the settings' `input_sounds`.
    inputs: Sounds<Sound<InputReader>>,
}

impl Renderer {
    /// Channels in every frame the pipeline makes, from the sources through
    /// to the slice `render` fills: always stereo. A mono source is heard on
    /// both; effects, when they come, can count on two.
    pub const CHANNELS: usize = 2;

    /// The most sample sounds any settings may ask for, and the default.
    /// A fixed count bounds the work in one audio callback; the cost per
    /// sound grows as playback gains pitch and processing, so a slower
    /// device asks for fewer.
    pub const MAX_SAMPLE_SOUNDS: usize = 64;

    /// The most input sounds any settings may ask for, and the default.
    /// Unlike samples, an input sound is never cut: `Control::input`
    /// refuses an input whose sounds would not all fit until one is
    /// removed.
    pub const MAX_INPUT_SOUNDS: usize = 8;

    /// The largest command ring any settings may ask for, and the default.
    /// A press that finds the ring full is dropped, which is the right
    /// failure for a real-time system; with periods in the hundreds per
    /// second and presses in the tens, it does not happen by hand.
    pub const MAX_COMMAND_CAPACITY: usize = 64;

    /// A silent renderer and the handle that commands it, as a pair, the
    /// way a channel comes as two ends. The renderer goes to the audio
    /// thread; the handle stays on the control side and is where every
    /// sound starts. Commands sent before the renderer is driven are not
    /// lost: they wait in the ring and apply on the first render. No audio
    /// device is involved: a machine without one drives it in a loop.
    ///
    /// Sized by `settings`: the period, the command ring and the places
    /// sounds are held in, all allocated here.
    pub fn new(settings: RendererSettings) -> (Renderer, Control) {
        let (commands_in, commands) = RingBuffer::new(settings.command_capacity);
        let (released_in, released) = RingBuffer::new(Self::released_capacity(&settings));
        (
            Self::on(commands, released_in, settings),
            Control::new(commands_in, released, settings.input_sounds),
        )
    }

    /// How many sounds can be let go of between two sends of the control,
    /// which empties the release ring before it pushes: every sound held,
    /// and one for every command that can be waiting, since each adds at
    /// most one sound and an add refused is let go of at once.
    fn released_capacity(settings: &RendererSettings) -> usize {
        settings.sample_sounds + settings.input_sounds + settings.command_capacity
    }

    /// A silent renderer sized by `settings` that takes its commands from
    /// `commands` and hands back what it lets go of on `released`; the
    /// rings' capacities are the caller's.
    pub(crate) fn on(
        commands: Consumer<Command>,
        released: Producer<Released>,
        settings: RendererSettings,
    ) -> Self {
        settings.debug_check();
        Self {
            commands,
            released,
            period: settings.period,
            samples: Sounds::new(settings.sample_sounds),
            inputs: Sounds::new(settings.input_sounds),
        }
    }

    /// Renders one period into `out`, an interleaved stereo slice,
    /// replacing whatever it held.
    ///
    /// Commands the handle has sent since the last call are applied first,
    /// so a play, stop or level change lands at a period boundary. Then
    /// every sample and every input adds into the zeroed slice at its
    /// gain, one that has finished is let go, and the sum is clipped once
    /// to the unit range. A sound at zero gain still runs, so an input
    /// nobody hears keeps up with its ring.
    pub fn render(&mut self, out: &mut [Pcm]) {
        let frames = out.len() / Self::CHANNELS;
        debug_assert!(frames <= self.period, "more frames than the period");
        self.apply_commands();
        out.fill(0.0);
        for sound in self.samples.iter_mut() {
            add(out, sound.frames(frames));
        }
        for sound in self.inputs.iter_mut() {
            add(out, sound.frames(frames));
        }
        let released = &mut self.released;
        self.samples
            .remove_where(|s| s.finished(), |s| release(released, s));
        self.inputs
            .remove_where(|s| s.finished(), |s| release(released, s));
        hard_clip(out);
    }

    /// Applies every command that has arrived, in order.
    fn apply_commands(&mut self) {
        let released = &mut self.released;
        while let Ok(command) = self.commands.pop() {
            match command {
                // A pad press always sounds, even if it cuts the oldest.
                Command::Sample(command) => apply(&mut self.samples, command, true, released),
                // An input already heard is never cut by another being
                // added. The handle refuses first; this is the backstop.
                Command::Input(command) => apply(&mut self.inputs, command, false, released),
                Command::StopAllSamples => self
                    .samples
                    .remove_where(|_| true, |s| release(released, s)),
                Command::Evict(addr) => self.samples.remove_where(
                    |s| BufferAddress::of(s.source().audio()) == addr,
                    |s| release(released, s),
                ),
            }
        }
    }

    /// Whether the sample or input sound `id` is in the renderer, heard
    /// or not.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: SoundId) -> bool {
        self.samples.contains(id) || self.inputs.contains(id)
    }

    /// How many sounds and inputs are heard.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.samples.len() + self.inputs.len()
    }
}

/// Applies `command` to `sounds`. An add that finds every place taken
/// cuts the oldest sound for it if `steal`, and is refused if not. What
/// leaves, cut or refused, goes to `released`.
fn apply<S>(
    sounds: &mut Sounds<Sound<S>>,
    command: SoundCommand<S>,
    steal: bool,
    released: &mut Producer<Released>,
) where
    Sound<S>: Into<Released>,
{
    match command {
        SoundCommand::Add { id, source, gain } => {
            let sound = Sound::new(source, Gain::at(gain));
            let Err(sound) = sounds.insert(id, sound) else {
                return;
            };
            let Some(oldest) = sounds.oldest().filter(|_| steal) else {
                return release(released, sound);
            };
            if let Some(cut) = sounds.remove(oldest) {
                release(released, cut);
            }
            if let Err(sound) = sounds.insert(id, sound) {
                release(released, sound);
            }
        }
        SoundCommand::Remove(id) => {
            if let Some(sound) = sounds.remove(id) {
                release(released, sound);
            }
        }
        SoundCommand::Set { id, param } => {
            if let Some(sound) = sounds.get_mut(id) {
                sound.set(param);
            }
        }
    }
}

/// Sends `sound` back to the control side to be dropped. The ring is
/// sized so it cannot be full; if it ever is, the sound is dropped here,
/// which is late but not wrong.
fn release(released: &mut Producer<Released>, sound: impl Into<Released>) {
    let pushed = released.push(sound.into());
    debug_assert!(pushed.is_ok(), "the release ring is sized never to fill");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::{Producer, RingBuffer};

    use super::*;
    use crate::audio::{AudioBuffer, ChannelLayout};
    use crate::render::command::SoundParam;
    use crate::testing::audio;

    const HALF: Pcm = 0.5;
    /// What these tests' renderers are sized with. The command ring is
    /// made by `renderer()` itself, eight deep, whatever this says.
    const SETTINGS: RendererSettings = RendererSettings::new(64);
    const FULL: Pcm = 1.0;

    fn renderer() -> (Producer<Command>, Consumer<Released>, Renderer) {
        renderer_with(SETTINGS)
    }

    /// A renderer, the command ring's sending end, and the release ring's
    /// receiving end, sized as `Renderer::new` sizes it.
    fn renderer_with(
        settings: RendererSettings,
    ) -> (Producer<Command>, Consumer<Released>, Renderer) {
        let (commands_in, commands) = RingBuffer::new(8);
        let (released_in, released) = RingBuffer::new(Renderer::released_capacity(&settings));
        (
            commands_in,
            released,
            Renderer::on(commands, released_in, settings),
        )
    }

    /// Drops everything the renderer has let go of, as the control does
    /// on each send, and says how many there were.
    fn collect(released: &mut Consumer<Released>) -> usize {
        let mut count = 0;
        while released.pop().is_ok() {
            count += 1;
        }
        count
    }

    fn mono(data: Vec<Pcm>) -> Arc<AudioBuffer> {
        Arc::new(audio(ChannelLayout::Mono, &data))
    }

    fn play(p: &mut Producer<Command>, id: u64, audio: &Arc<AudioBuffer>) {
        p.push(
            SoundCommand::Add {
                id: SoundId(id),
                source: SampleReader::new(audio.clone()),
                gain: 1.0,
            }
            .into(),
        )
        .unwrap();
    }

    /// Adds a stereo input sound `id` and hands back the writer that
    /// feeds it.
    fn add_input(p: &mut Producer<Command>, id: u64) -> Producer<Pcm> {
        let (writer, reader) = RingBuffer::new(8 * 2);
        p.push(
            SoundCommand::Add {
                id: SoundId(id),
                source: InputReader::new(reader, ChannelLayout::Stereo),
                gain: 0.0,
            }
            .into(),
        )
        .unwrap();
        writer
    }

    fn set_input_gain(p: &mut Producer<Command>, id: u64, level: f32) {
        p.push(
            SoundCommand::<InputReader>::Set {
                id: SoundId(id),
                param: SoundParam::Gain(level),
            }
            .into(),
        )
        .unwrap();
    }

    #[test]
    fn silence_when_nothing_plays() {
        let (_p, _released, mut r) = renderer();
        let mut out = vec![0.7; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn a_play_is_heard_from_the_next_render() {
        let (mut p, _released, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF, HALF]));
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.5; 4]);
    }

    #[test]
    fn sounds_sum_and_the_sum_is_clipped() {
        let (mut p, _released, mut r) = renderer();
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
    fn an_input_at_zero_gain_is_silent_but_drained_and_a_gain_ramps_in() {
        let (mut p, mut released, mut r) = renderer();
        play(&mut p, 1, &mono(vec![HALF; 100]));
        // A stereo input sound over a ring of eight frames.
        let (mut writer, reader) = RingBuffer::new(8 * 2);
        p.push(
            SoundCommand::Add {
                id: SoundId(3),
                source: InputReader::new(reader, ChannelLayout::Stereo),
                gain: 0.0,
            }
            .into(),
        )
        .unwrap();
        feed_two(&mut writer);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(out, vec![0.5; 4], "an input at zero gain is not heard");
        assert_eq!(r.playing(), 2);
        assert_eq!(writer.slots(), 8 * 2, "but it keeps up with its ring");

        // Up to full over the two frames of the period: half, then all.
        set_input_gain(&mut p, 3, 1.0);
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.625, 0.375, 0.75, 0.25]);

        // A sample remove under the input's id and a stop-all leave the
        // input alone.
        p.push(SoundCommand::<SampleReader>::Remove(SoundId(3)).into())
            .unwrap();
        p.push(Command::StopAllSamples).unwrap();
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.25, -0.25, 0.25, -0.25]);
        assert_eq!(collect(&mut released), 1, "only the sample came back");

        // Down to nothing the same way.
        set_input_gain(&mut p, 3, 0.0);
        feed_two(&mut writer);
        r.render(&mut out);
        assert_eq!(out, vec![0.125, -0.125, 0.0, 0.0]);
        assert!(r.is_playing(SoundId(3)), "silent, still an input");

        p.push(SoundCommand::<InputReader>::Remove(SoundId(3)).into())
            .unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(SoundId(3)));
        // The reader is on its way back, not dropped on the audio thread.
        assert!(!writer.is_abandoned());
        // Dropped once the control side collects, so the writer feeds nothing.
        assert_eq!(collect(&mut released), 1);
        assert!(writer.is_abandoned());
    }

    #[test]
    fn stop_and_evict_land_at_the_next_render() {
        let (mut p, mut released, mut r) = renderer();
        let a = mono(vec![HALF; 100]);
        let b = mono(vec![HALF; 100]);
        play(&mut p, 1, &a);
        play(&mut p, 2, &b);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 2);

        p.push(SoundCommand::<SampleReader>::Remove(SoundId(1)).into())
            .unwrap();
        r.render(&mut out);
        assert!(!r.is_playing(SoundId(1)));
        assert_eq!(out, vec![0.5; 4]);

        p.push(Command::Evict(BufferAddress::of(&b))).unwrap();
        r.render(&mut out);
        assert_eq!(r.playing(), 0);
        assert_eq!(out, vec![0.0; 4]);
        assert_eq!(collect(&mut released), 2, "both came back");
    }

    #[test]
    fn a_sound_holding_the_last_reference_frees_it_when_collected_not_on_the_render() {
        let (mut p, mut released, mut r) = renderer();
        let audio = mono(vec![HALF; 3]);
        play(&mut p, 1, &audio);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        // The caller lets go first: the sound now holds the only reference.
        let watch = Arc::downgrade(&audio);
        drop(audio);

        r.render(&mut out);
        assert!(!r.is_playing(SoundId(1)), "finished");
        assert!(watch.upgrade().is_some(), "not freed on the audio thread");
        assert_eq!(collect(&mut released), 1);
        assert!(watch.upgrade().is_none(), "freed by the collect");
    }

    #[test]
    fn a_stolen_sound_comes_back() {
        let (mut p, mut released, mut r) = renderer_with(RendererSettings {
            sample_sounds: 1,
            ..SETTINGS
        });
        let a = mono(vec![HALF; 100]);
        play(&mut p, 1, &a);
        play(&mut p, 2, &a);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert!(r.is_playing(SoundId(2)));
        assert_eq!(collect(&mut released), 1, "the cut sound");
    }

    #[test]
    fn a_sample_added_when_full_cuts_the_oldest() {
        let (mut p, _released, mut r) = renderer_with(RendererSettings {
            sample_sounds: 2,
            ..SETTINGS
        });
        let a = mono(vec![HALF; 100]);
        play(&mut p, 1, &a);
        play(&mut p, 2, &a);
        play(&mut p, 3, &a);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert!(!r.is_playing(SoundId(1)), "the oldest is cut");
        assert!(r.is_playing(SoundId(2)) && r.is_playing(SoundId(3)));
    }

    #[test]
    fn a_freed_place_is_taken_before_anything_is_cut() {
        let (mut p, _released, mut r) = renderer_with(RendererSettings {
            sample_sounds: 2,
            ..SETTINGS
        });
        let a = mono(vec![HALF; 100]);
        play(&mut p, 1, &a);
        play(&mut p, 2, &a);
        p.push(SoundCommand::<SampleReader>::Remove(SoundId(1)).into())
            .unwrap();
        play(&mut p, 3, &a);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert!(r.is_playing(SoundId(2)), "nothing playing was cut");
        assert!(r.is_playing(SoundId(3)));
        assert_eq!(r.playing(), 2);
    }

    #[test]
    fn an_input_added_when_full_is_dropped_and_none_is_cut() {
        let (mut p, mut released, mut r) = renderer_with(RendererSettings {
            input_sounds: 1,
            ..SETTINGS
        });
        let first = add_input(&mut p, 1);
        let second = add_input(&mut p, 2);
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert!(r.is_playing(SoundId(1)), "the input already held stays");
        assert!(!r.is_playing(SoundId(2)));
        assert!(!first.is_abandoned());
        assert_eq!(collect(&mut released), 1, "the refused input came back");
        assert!(second.is_abandoned(), "and its reader is gone");
    }
}
