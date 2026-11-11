//! The control side's end of the renderer: what you hold here to make the
//! renderer, on its own thread, do something.
//!
//! [`Renderer::new`] mints the renderer and the command ring between them
//! and returns the two ends the way a channel does: the [`Renderer`] goes
//! to the audio thread, the [`Control`] stays. Everything heard is a verb
//! on it: `play` a loaded sample, `stop` its sounds, add an `input`,
//! `stop_all`. What those return, a [`Sound`], an [`Input`] and its
//! sources, carries a clone of the `Control` so its own verbs work without
//! the parent. It is cheap to clone and holds nothing but the ring's
//! sending end, the sound id counter and the input slot count, behind one
//! `Arc`.
//!
//! Building the [`Command`] that carries each verb is this type's job and
//! nobody else's, so the command vocabulary is private between here and
//! the renderer. Every send goes through one lock, on the control side
//! only. The renderer's end of the ring never blocks, and the renderer
//! never sees a control, only the commands. Nothing that runs on the audio
//! or capture thread may hold one of these: it has a mutex.
//!
//! The control mints [`SoundId`]s, because a sound's id has to be in the
//! command that starts it and every holder must agree on the names: one
//! counter, only going up, so no id is ever reused. A command refused for
//! want of room leaves its id burnt; ids are plentiful and nothing reads
//! the gaps.
//!
//! The control knows nothing about projects or disk. A [`LoadedSample`]
//! comes from a project and is played here by reference; the two sides
//! share nothing but the ledger and that value, carried by the caller.
//!
//! [`Renderer::new`]: crate::render::Renderer::new
//! [`Renderer`]: crate::render::Renderer

pub(crate) mod input;
pub(crate) mod sound;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::Producer;
use thiserror::Error;

use crate::config::{COMMAND_CAPACITY, MAX_INPUT_CHANNELS, MAX_INPUT_SOUNDS, MAX_PERIOD_FRAMES};
use crate::control::sound::Sound;
use crate::project::loaded_sample::LoadedSample;
use crate::render::command::{Command, InputCommand, SampleCommand};
use crate::render::dsp::{InputReader, SoundId};
use input::{ChannelLayout, Input, InputFeed};

/// Reasons the control side can refuse what it is asked: the ring is full,
/// or an input would not fit.
#[derive(Debug, Error)]
pub enum ControlError {
    /// The command ring was full; the command was dropped.
    #[error("the renderer has not taken the last {COMMAND_CAPACITY} commands")]
    CommandsFull,
    /// The layout asked of `input` adds up to no channels, or more than
    /// the renderer takes from one device.
    #[error("an input is 1 to {MAX_INPUT_CHANNELS} channels wide, not {0}")]
    BadInputWidth(usize),
    /// The capture period given to `input` is zero or longer than the
    /// renderer takes.
    #[error("a capture period is 1 to {MAX_PERIOD_FRAMES} frames, not {0}")]
    BadInputPeriod(usize),
    /// The input's sources would not all fit beside the input sounds
    /// already held. Drop an input before adding another.
    #[error("the renderer holds {MAX_INPUT_SOUNDS} input sounds at most")]
    InputsFull,
}

/// The control side's end of the renderer, from
/// [`Renderer::new`](crate::render::Renderer::new). Everything heard
/// starts as a verb here; cheap to clone, and cloned into every sound and
/// input it hands out.
#[derive(Debug, Clone)]
pub struct Control(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    /// The sending end of the command ring.
    commands: Mutex<Producer<Command>>,
    /// The next sound id. Only goes up.
    next_sound: AtomicU64,
    /// Input sounds added and not yet removed: the slots of the renderer's
    /// input mixer that are spoken for. The control side's answer to "is
    /// there room", so that question is never put to the render side,
    /// which would answer by silently rejecting the add.
    input_sounds: AtomicUsize,
}

impl Control {
    /// A control sending over `commands`, minting sound ids from zero.
    pub(crate) fn new(commands: Producer<Command>) -> Self {
        Self(Arc::new(Shared {
            commands: Mutex::new(commands),
            next_sound: AtomicU64::new(0),
            input_sounds: AtomicUsize::new(0),
        }))
    }

    /// Starts playing `sample` from its first frame and returns the sound,
    /// which is how it is stopped: [`Sound::stop`]. The sound shares the
    /// sample's audio; nothing is copied or read from disk, so a press
    /// costs the same every time. Sound starts at the renderer's next
    /// period. A play never fails for want of a free slot: the renderer
    /// steals the oldest. Refused as `CommandsFull` if the ring has no
    /// room.
    pub fn play(&self, sample: &LoadedSample) -> Result<Sound, ControlError> {
        let id = self.mint(1);
        self.send(SampleCommand::Play {
            id,
            audio: Arc::clone(sample.audio()),
        })?;
        Ok(Sound::new(id, self.clone()))
    }

    /// Stops every sound playing `sample`, hard, at the renderer's next
    /// period. Unloading is dropping the sample; its sounds then play out,
    /// each letting go of the audio as it ends, so call this first if they
    /// should not. Refused as `CommandsFull` if the ring has no room.
    ///
    /// The sample is taken by reference, so its address is still its own
    /// when the eviction is pushed and cannot name a later buffer; see
    /// `BufferAddress`.
    pub fn stop(&self, sample: &LoadedSample) -> Result<(), ControlError> {
        self.send(SampleCommand::Evict(sample.addr()))
    }

    /// Stops every sample at the renderer's next period. Inputs keep
    /// being heard. Refused as `CommandsFull` if the ring has no room.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.send(SampleCommand::StopAll)
    }

    /// Adds an input: the ability to put audio into the renderer. Mints a
    /// ring per source, sends the reading ends to the render side, and
    /// returns the control and the feed. The [`Input`] is yours to hold:
    /// its [`sources`](Input::sources) are what you monitor, and dropping
    /// it removes the input. The [`InputFeed`] goes to whoever owns the
    /// capture thread, to feed from a device's input callback. Nothing is
    /// heard until a source is monitored.
    ///
    /// An input is shaped like the device it comes from. `layout` says
    /// what each channel of the device's interleaved frames means, in
    /// order: a `Stereo` is the next two channels heard left and right, a
    /// `Mono` is the next one heard on both sides. The frames fed are as
    /// wide as the layout adds up to, 1 to `MAX_INPUT_CHANNELS`, exactly
    /// as the device delivered them. Each source is a sound of its own,
    /// its own ring split out of the device frames by [`InputFeed::feed`],
    /// so the render side sees mono and stereo sounds and nothing wider.
    /// The layout is fixed for the input's life: a device that is to
    /// deliver other channels is closed and reopened, and so is its input.
    /// Up to `MAX_INPUT_SOUNDS` sources at once across every input; an
    /// input whose sources would not all fit is refused with `InputsFull`
    /// until one is dropped, so an input already heard is never cut by
    /// another being added.
    ///
    /// `period` is how many frames the capture device delivers per
    /// callback, which the caller knows from opening it and the renderer
    /// cannot: the rings are sized from it, a few periods deep, so the
    /// capture and output clocks have room to disagree before a frame is
    /// dropped. At most `MAX_PERIOD_FRAMES`, which is what makes the rings
    /// fixed overhead with a known worst case rather than audio-sized
    /// memory on the ledger.
    pub fn input(
        &self,
        layout: &[ChannelLayout],
        period: usize,
    ) -> Result<(Input, InputFeed), ControlError> {
        let width = input::width(layout);
        if !(1..=MAX_INPUT_CHANNELS).contains(&width) {
            return Err(ControlError::BadInputWidth(width));
        }
        if !(1..=MAX_PERIOD_FRAMES).contains(&period) {
            return Err(ControlError::BadInputPeriod(period));
        }
        // A sound per source. The capture callback writes frames in
        // through the feed, the renderer reads them out through the
        // readers on the audio thread. One command per source; all of
        // them go or none does. The sound ids a refused add minted are
        // burnt, and nothing minds.
        let first = self.mint(layout.len());
        let sounds: Vec<SoundId> = (0..layout.len() as u64)
            .map(|n| SoundId(first.0 + n))
            .collect();
        let (feed, readers) = InputFeed::new(layout, period);
        self.add_input(&sounds, readers)?;
        Ok((Input::new(&sounds, self.clone()), feed))
    }

    /// Stops the sound `id` if it is still playing.
    pub(crate) fn stop_sound(&self, id: SoundId) -> Result<(), ControlError> {
        self.send(SampleCommand::Stop(id))
    }

    /// `count` fresh, consecutive sound ids; the first is returned.
    fn mint(&self, count: usize) -> SoundId {
        SoundId(self.0.next_sound.fetch_add(count as u64, Ordering::Relaxed))
    }

    /// Adds the sources of an input, the sound `sounds[n]` reading
    /// `readers[n]`, none of them heard yet, taking a slot of the input
    /// mixer for each. All are added or none is: refused as `InputsFull`
    /// if they would not all fit beside the input sounds already held, so
    /// an input already heard is never cut by another being added, and as
    /// `CommandsFull` with nothing sent and no slot taken if the ring has
    /// no room for every one.
    fn add_input(&self, sounds: &[SoundId], readers: Vec<InputReader>) -> Result<(), ControlError> {
        debug_assert_eq!(sounds.len(), readers.len(), "a sound per reader");
        let count = sounds.len();
        self.0
            .input_sounds
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                (held + count <= MAX_INPUT_SOUNDS).then_some(held + count)
            })
            .map_err(|_| ControlError::InputsFull)?;
        let sent = self.send_all(
            sounds
                .iter()
                .zip(readers)
                .map(|(&id, reader)| InputCommand::Add { id, reader }.into()),
        );
        if sent.is_err() {
            self.0.input_sounds.fetch_sub(count, Ordering::Relaxed);
        }
        sent
    }

    /// Removes the input sounds `sounds`, dropping their readers, and gives
    /// their slots back. All or none, as [`add_input`](Self::add_input);
    /// a refused remove keeps the slots spoken for.
    pub(crate) fn remove_input(&self, sounds: &[SoundId]) -> Result<(), ControlError> {
        self.send_all(sounds.iter().map(|&id| InputCommand::Remove(id).into()))?;
        self.0
            .input_sounds
            .fetch_sub(sounds.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Hears the input sound `id` through the output, or stops hearing it:
    /// the input mixer's mute, which `Mixer::set_muted` is the other end of.
    pub(crate) fn set_monitored(&self, id: SoundId, monitored: bool) -> Result<(), ControlError> {
        self.send(if monitored {
            InputCommand::Monitor(id)
        } else {
            InputCommand::Unmonitor(id)
        })
    }

    /// How many input sounds are held: the input mixer's slots spoken for.
    #[cfg(test)]
    pub(crate) fn input_sounds(&self) -> usize {
        self.0.input_sounds.load(Ordering::Relaxed)
    }

    /// The next id [`mint`](Self::mint) would return.
    #[cfg(test)]
    pub(crate) fn next_sound(&self) -> u64 {
        self.0.next_sound.load(Ordering::Relaxed)
    }

    /// Pushes `command`. A full ring drops it and says so.
    fn send(&self, command: impl Into<Command>) -> Result<(), ControlError> {
        self.lock()
            .push(command.into())
            .map_err(|_| ControlError::CommandsFull)
    }

    /// Pushes every command of `commands`, or none of them: refused as
    /// `CommandsFull` with nothing sent if the ring has no room for all.
    /// The lock is held across the check and the pushes, so no other
    /// holder can take the room in between.
    fn send_all(
        &self,
        commands: impl ExactSizeIterator<Item = Command>,
    ) -> Result<(), ControlError> {
        let mut ring = self.lock();
        if ring.slots() < commands.len() {
            return Err(ControlError::CommandsFull);
        }
        for command in commands {
            ring.push(command).expect("room was checked under the lock");
        }
        Ok(())
    }

    /// The ring. A panic while holding the lock leaves the ring itself
    /// intact, so a poisoned lock is taken anyway.
    fn lock(&self) -> MutexGuard<'_, Producer<Command>> {
        self.0
            .commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::Pcm;
    use crate::project::tests::memory_project;
    use crate::project::{Project, SampleId};
    use crate::render::Renderer;
    use crate::testing::audio;

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        audio(1, &(0..n).map(|i| i as Pcm * 0.01).collect::<Vec<_>>())
    }

    /// What `ramp(n)` costs the budget.
    fn ramp_bytes(n: usize) -> usize {
        n * size_of::<Pcm>()
    }

    /// A project on in-memory storage with a ramp of `n` frames in it, and
    /// that sample's id.
    fn project_with(n: usize) -> (Project, SampleId) {
        let mut p = memory_project();
        let id = p.add_sample(None, &ramp(n)).unwrap();
        (p, id)
    }

    /// Renders one period of `frames` frames and returns it.
    fn period(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        r.render(&mut out);
        out
    }

    #[test]
    fn commands_sent_before_the_renderer_is_driven_wait_for_it() {
        let (p, id) = project_with(2);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();
        control.play(&loaded).unwrap();

        let out = period(&mut r, 2);
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn play_is_heard_from_the_renderers_next_period() {
        let (p, id) = project_with(2);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();

        let play = control.play(&loaded).unwrap();
        let out = period(&mut r, 2);
        // Two frames fit in one two-frame period, so the sound is done.
        assert!(!r.is_playing(play.id()));
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn each_play_is_its_own_sound() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();

        let a = control.play(&loaded).unwrap();
        let b = control.play(&loaded).unwrap();
        assert_ne!(a.id(), b.id());
        period(&mut r, 4);
        assert_eq!(r.playing(), 2);

        // The verb is on the sound, not the control.
        a.stop().unwrap();
        period(&mut r, 4);
        assert!(!r.is_playing(a.id()));
        assert!(r.is_playing(b.id()));

        control.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn a_full_ring_refuses_the_command_and_the_next_play_is_a_new_sound() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();
        let mut last = None;
        for _ in 0..COMMAND_CAPACITY {
            last = Some(control.play(&loaded).unwrap());
        }
        assert!(matches!(
            control.play(&loaded),
            Err(ControlError::CommandsFull)
        ));
        assert!(matches!(
            control.stop_all(),
            Err(ControlError::CommandsFull)
        ));

        // Once the renderer has drained the ring, plays go again. The
        // refused press burnt an id, which nothing minds; the next sound
        // is simply a later one.
        period(&mut r, 4);
        let next = control.play(&loaded).unwrap();
        assert!(next.id() > last.unwrap().id());
    }

    #[test]
    fn an_input_is_one_to_max_channels_wide_and_has_a_period() {
        let (_r, control) = Renderer::new();
        let too_wide = [ChannelLayout::Mono; MAX_INPUT_CHANNELS + 1];
        for bad in [&[][..], &too_wide[..]] {
            assert!(matches!(
                control.input(bad, 64),
                Err(ControlError::BadInputWidth(w)) if w == bad.len()
            ));
        }
        for bad in [0, MAX_PERIOD_FRAMES + 1] {
            assert!(matches!(
                control.input(&[ChannelLayout::Mono], bad),
                Err(ControlError::BadInputPeriod(p)) if p == bad
            ));
        }
        assert_eq!(control.input_sounds(), 0, "a refused input holds no slot");
    }

    #[test]
    fn each_source_of_an_input_is_heard_on_its_own_and_all_go_when_it_is_dropped() {
        let (mut r, control) = Renderer::new();
        // A device of four channels: a stereo pair, then two microphones.
        let (input, mut feed) = control
            .input(
                &[
                    ChannelLayout::Stereo,
                    ChannelLayout::Mono,
                    ChannelLayout::Mono,
                ],
                64,
            )
            .unwrap();
        let [pair, mic_a, mic_b] = input.sources() else {
            panic!("three sources");
        };
        assert_eq!(feed.channels(), 4);
        assert_eq!(control.input_sounds(), 3);
        // One four-wide frame per feed, quiet enough that no sum clips.
        let frame = [0.1, 0.2, 0.3, 0.4];

        feed.feed(&frame);
        period(&mut r, 1);
        assert!(
            [pair, mic_a, mic_b].iter().all(|s| r.is_playing(s.id())),
            "an input's sounds are in the renderer at once"
        );

        // The verbs are on the source, not the control.
        pair.monitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.1, 0.2]);

        mic_b.monitor().unwrap();
        pair.unmonitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.4, 0.4]);

        mic_a.monitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.3 + 0.4, 0.3 + 0.4]);

        let ids: Vec<SoundId> = input.sources().iter().map(|s| s.id()).collect();
        drop(input);
        assert_eq!(control.input_sounds(), 0, "the slots are given back");
        period(&mut r, 1);
        assert_eq!(r.playing(), 0, "every source of the input is gone");
        assert!(ids.iter().all(|&s| !r.is_playing(s)));
        // The feed outlives the control harmlessly: its frames go nowhere.
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.0, 0.0]);
    }

    #[test]
    fn more_input_sounds_than_slots_are_refused_until_an_input_is_dropped() {
        let (mut r, control) = Renderer::new();
        // One stereo input, then mono inputs up to the slot count.
        let (first, _first_feed) = control
            .input(&[ChannelLayout::Stereo, ChannelLayout::Mono], 64)
            .unwrap();
        let rest: Vec<_> = (2..MAX_INPUT_SOUNDS)
            .map(|_| control.input(&[ChannelLayout::Mono], 64).unwrap())
            .collect();
        assert!(matches!(
            control.input(&[ChannelLayout::Mono], 64),
            Err(ControlError::InputsFull)
        ));
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);

        // Dropping the two-source input makes room for two sources, not three.
        let gone: Vec<SoundId> = first.sources().iter().map(|s| s.id()).collect();
        drop(first);
        assert!(matches!(
            control.input(
                &[
                    ChannelLayout::Mono,
                    ChannelLayout::Mono,
                    ChannelLayout::Mono
                ],
                64
            ),
            Err(ControlError::InputsFull)
        ));
        let (next, _next_feed) = control
            .input(&[ChannelLayout::Mono, ChannelLayout::Mono], 64)
            .unwrap();
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);
        assert!(gone.iter().all(|&s| !r.is_playing(s)));
        assert!(next.sources().iter().all(|s| r.is_playing(s.id())));
        assert!(rest.iter().all(|(i, _)| r.is_playing(i.sources()[0].id())));
    }

    #[test]
    fn a_full_ring_refuses_an_input_and_holds_no_slot() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();
        for _ in 0..COMMAND_CAPACITY {
            control.play(&loaded).unwrap();
        }
        assert!(matches!(
            control.input(&[ChannelLayout::Stereo], 64),
            Err(ControlError::CommandsFull)
        ));
        assert_eq!(control.input_sounds(), 0);

        // With room again the input is added, its sources consecutive.
        period(&mut r, 4);
        let before = control.next_sound();
        let (input, _feed) = control
            .input(&[ChannelLayout::Stereo, ChannelLayout::Mono], 64)
            .unwrap();
        let ids: Vec<SoundId> = input.sources().iter().map(|s| s.id()).collect();
        assert_eq!(ids, [SoundId(before), SoundId(before + 1)]);
        assert_eq!(control.next_sound(), before + 2);
        assert_eq!(control.input_sounds(), 2);
    }

    #[test]
    fn an_input_dropped_onto_a_full_ring_keeps_its_slots_until_its_feed_goes() {
        let (mut r, control) = Renderer::new();
        let (input, feed) = control.input(&[ChannelLayout::Mono], 64).unwrap();
        let sound = input.sources()[0].id();
        period(&mut r, 1);
        while control.stop_all().is_ok() {}

        // No room for the remove: the drop cannot fail, so it degrades. The
        // reader stays and the slot stays spoken for.
        drop(input);
        assert_eq!(control.input_sounds(), 1);
        period(&mut r, 1);
        assert!(r.is_playing(sound));

        // Dropping the feed finishes the reader on its own.
        drop(feed);
        period(&mut r, 1);
        assert!(!r.is_playing(sound));
    }

    #[test]
    fn stop_cuts_the_sounds_playing_the_sample_and_no_other() {
        let mut p = memory_project();
        let gone = p.add_sample(None, &ramp(100)).unwrap();
        let kept = p.add_sample(None, &ramp(100)).unwrap();
        let gone = p.load(gone).unwrap();
        let kept = p.load(kept).unwrap();
        let (mut r, control) = Renderer::new();
        let a = control.play(&gone).unwrap();
        let b = control.play(&kept).unwrap();
        period(&mut r, 4);

        control.stop(&gone).unwrap();
        // Gone from the output at its next render, and only that sample.
        period(&mut r, 4);
        assert!(!r.is_playing(a.id()));
        assert!(r.is_playing(b.id()));
    }

    #[test]
    fn stop_then_drop_frees_the_audio_once_the_sounds_are_cut() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();
        let play = control.play(&loaded).unwrap();
        period(&mut r, 4);

        control.stop(&loaded).unwrap();
        drop(loaded);
        // The sound still holds the audio until the renderer applies the
        // eviction at its next render; then both are gone.
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        period(&mut r, 4);
        assert!(!r.is_playing(play.id()));
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn stop_on_a_full_ring_is_refused_and_the_sample_is_untouched() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (_r, control) = Renderer::new();
        for _ in 0..COMMAND_CAPACITY {
            control.play(&loaded).unwrap();
        }

        assert!(matches!(
            control.stop(&loaded),
            Err(ControlError::CommandsFull)
        ));
        assert_eq!(loaded.id(), id);
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
    }

    #[test]
    fn a_dropped_sample_lets_its_sounds_play_out() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new();
        let play = control.play(&loaded).unwrap();
        period(&mut r, 4);

        drop(loaded);
        // Nothing was sent: the sound keeps the audio alive and plays on.
        period(&mut r, 4);
        assert!(r.is_playing(play.id()));
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        // The audio is let go when the sound is.
        control.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(p.ledger().in_use(), 0);
    }
}
