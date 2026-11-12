//! The control side's end of the renderer: what you hold here to make the
//! renderer, on its own thread, do something.
//!
//! [`Renderer::new`] mints the renderer and the command ring between them
//! and returns the two ends the way a channel does: the [`Renderer`] goes
//! to the audio thread, the [`Control`] stays. Everything heard is a verb
//! on it: `play` a loaded sample, `stop` its sounds, add an `input`,
//! `stop_all`. What those return, a [`SampleSound`], an [`Input`] and its
//! sounds, carries a clone of the `Control` so its own verbs work without
//! the parent. It is cheap to clone and holds nothing but its channel to
//! the renderer (its ends of the two rings), the sound id counter, the
//! count of input sounds held and a clone of the ledger, behind one
//! `Arc`.
//!
//! Building the [`Command`] that carries each verb is this type's job and
//! nobody else's, so the command vocabulary is private between here and
//! the renderer. Every send goes through one lock, on the control side
//! only. The renderer's end of the ring never blocks, and the renderer
//! never sees a control, only the commands. Nothing that runs on the audio
//! or capture thread may hold one of these: it has a mutex.
//!
//! The renderer sends back every sound it lets go of, and the control
//! drops them here, on the control side; a take it sends back is kept,
//! by its take, for its owner to take back. This happens
//! whenever the channel is taken, so at the start of every send, and
//! whenever [`collect`](Control::collect) is called. A sample's audio the
//! caller has already dropped is freed then. The ring cannot fill,
//! because nothing reaches the renderer except through a send, and every
//! send empties it first.
//!
//! The control mints [`SoundId`]s, because a sound's id has to be in the
//! command that starts it and every holder must agree on the names: one
//! counter, only going up, so no id is ever reused. A command refused for
//! want of room leaves its id burnt; ids are plentiful and nothing reads
//! the gaps.
//!
//! The control knows nothing about projects or disk. A [`Sample`]
//! comes from a project and is played here by reference; the two sides
//! share nothing but the ledger and that value, carried by the caller.
//!
//! [`Renderer::new`]: crate::render::Renderer::new
//! [`Renderer`]: crate::render::Renderer

pub(crate) mod input;
pub(crate) mod recorder;
pub(crate) mod sample_sound;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer};
use thiserror::Error;

use crate::audio::{AudioBuffer, ChannelLayout, Take};
use crate::config::{MAX_INPUT_CHANNELS, MAX_PERIOD_FRAMES};
use crate::control::sample_sound::SampleSound;
use crate::memory::{Ledger, OverBudgetError};
use crate::project::sample::Sample;
use crate::render::Renderer;
use crate::render::command::{
    Command, InputRing, Released, SoundCommand, SoundId, SoundParam, TakeId, Tap,
};
use input::{Input, InputFeed, InputLayout};
use recorder::Recorder;

/// Reasons the control side can refuse what it is asked: the ring is full,
/// an input would not fit, or a recorder cannot be had or started.
#[derive(Debug, Error)]
pub enum ControlError {
    /// The command ring was full; the command was dropped.
    #[error("the command ring is full: the renderer has not taken the commands sent")]
    CommandsFull,
    /// The layout asked of `input` adds up to no channels, or more than
    /// the renderer takes from one device.
    #[error("an input is 1 to {MAX_INPUT_CHANNELS} channels wide, not {0}")]
    BadInputWidth(usize),
    /// The capture period given to `input` is zero or longer than the
    /// renderer takes.
    #[error("a capture period is 1 to {MAX_PERIOD_FRAMES} frames, not {0}")]
    BadInputPeriod(usize),
    /// The input's sounds would not all fit beside the input sounds
    /// already held. Drop an input before adding another.
    #[error("the renderer holds as many input sounds as it can")]
    InputsFull,
    /// There is already a recorder. Drop it before making another.
    #[error("there is already a recorder")]
    RecorderInUse,
    /// A recorder's buffer would put audio memory over its budget.
    #[error(transparent)]
    OverBudget(#[from] OverBudgetError),
    /// A take is already running on this recorder. Stop it first.
    #[error("the recorder is already recording")]
    Recording,
}

/// The control side's end of the renderer, from
/// [`Renderer::new`](crate::render::Renderer::new). Everything heard
/// starts as a verb here; cheap to clone, and cloned into every sound and
/// input it hands out.
#[derive(Debug, Clone)]
pub struct Control(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    /// The way to and from the renderer, under one lock.
    channel: Mutex<RendererChannel>,
    /// The next sound id. Only goes up.
    next_sound: AtomicU64,
    /// Input sounds added and not yet removed: the renderer's places for
    /// input sounds that are spoken for. The control side's answer to "is
    /// there room", so that question is never put to the render side,
    /// which would answer by silently rejecting the add.
    input_sounds: AtomicUsize,
    /// How many input sounds the renderer holds at once.
    input_capacity: usize,
    /// The next take id. Only goes up.
    next_take: AtomicU64,
    /// Whether the one recorder has been made and not yet dropped.
    recorder_held: AtomicBool,
    /// What the control pays from for what it builds and sends to the
    /// renderer. The render side never holds one.
    ledger: Ledger,
}

/// The control's channel to the renderer: every command goes out here and
/// everything the renderer hands back comes in here. Its ends of the two
/// rings, the command ring out and the release ring back, are locked
/// together so a send empties the release ring and pushes as one step,
/// and the takes that came back wait here for their owners.
#[derive(Debug)]
struct RendererChannel {
    /// The sending end of the command ring.
    commands: Producer<Command>,
    /// The receiving end of the ring the renderer sends back every sound
    /// and recorder it lets go of on.
    released: Consumer<Released>,
    /// Takes the renderer has sent back, each in the buffer it was
    /// recorded into, by take, until their owner takes them back.
    takes: HashMap<TakeId, AudioBuffer>,
    /// Recordings whose owner is gone: their take is dropped when it
    /// comes back instead of kept.
    abandoned: HashSet<TakeId>,
}

impl RendererChannel {
    /// Takes everything the renderer has let go of since the last
    /// collect: drops what every sound was made from, a sample's audio or
    /// an input's ring, and keeps every take for its owner
    /// unless the owner is gone.
    fn collect(&mut self) {
        while let Ok(released) = self.released.pop() {
            if let Released::Take { id, audio } = released
                && !self.abandoned.remove(&id)
            {
                self.takes.insert(id, audio);
            }
        }
    }
}

impl Control {
    /// A control sending over `commands` and dropping what comes back on
    /// `released`, minting sound ids from zero, for a renderer that holds
    /// `input_capacity` input sounds, paying from `ledger`.
    pub(crate) fn new(
        commands: Producer<Command>,
        released: Consumer<Released>,
        input_capacity: usize,
        ledger: Ledger,
    ) -> Self {
        Self(Arc::new(Shared {
            channel: Mutex::new(RendererChannel {
                commands,
                released,
                takes: HashMap::new(),
                abandoned: HashSet::new(),
            }),
            next_sound: AtomicU64::new(0),
            input_sounds: AtomicUsize::new(0),
            input_capacity,
            next_take: AtomicU64::new(0),
            recorder_held: AtomicBool::new(false),
            ledger,
        }))
    }

    /// Starts playing `sample` from its first frame and returns the sound,
    /// which is how it is stopped: [`SampleSound::stop`]. The sound shares the
    /// sample's audio; nothing is copied or read from disk, so a press
    /// costs the same every time. The sound starts at the renderer's next
    /// period. A play never fails for want of room: the renderer cuts
    /// the oldest. Refused as `CommandsFull` if the ring has no
    /// room.
    pub fn play(&self, sample: &Sample) -> Result<SampleSound, ControlError> {
        let id = self.mint(1);
        self.send(SoundCommand::Add {
            id,
            from: Arc::clone(sample.audio()),
            gain: 1.0,
        })?;
        Ok(SampleSound::new(id, self.clone()))
    }

    /// Stops every sound playing `sample`, hard, at the renderer's next
    /// period. Unloading is dropping the sample; its sounds then play out,
    /// each letting go of the audio as it ends, so call this first if they
    /// should not. Refused as `CommandsFull` if the ring has no room.
    ///
    /// The sample is taken by reference, so its address is still its own
    /// when the eviction is pushed and cannot name a later buffer; see
    /// `BufferAddress`.
    pub fn stop(&self, sample: &Sample) -> Result<(), ControlError> {
        self.send(Command::Evict(sample.addr()))
    }

    /// Stops every sample at the renderer's next period. Inputs keep
    /// being heard. Refused as `CommandsFull` if the ring has no room.
    pub fn stop_all(&self) -> Result<(), ControlError> {
        self.send(Command::StopAllSamples)
    }

    /// Frees, here on the control side, every sound the renderer has let
    /// go of since the last send or collect: the audio of a sample dropped
    /// while it played, once that sound has ended, and the rings of an
    /// input removed. A take, full or stopped, is kept for its owner
    /// instead. Every send does this first, so only an owner that can go
    /// quiet needs it: call it from a UI or event loop tick, or
    /// before a load that needs the budget back. Never on the audio
    /// thread: it is where the freeing happens.
    pub fn collect(&self) {
        // Taking the channel is the collect (see `channel`), so take it and let
        // it go: there is nothing to send.
        drop(self.channel());
    }

    /// The recorder, with room for `frames` stereo frames, paid for now
    /// from the ledger, so no take ever allocates, and every page of it
    /// touched now, so recording never waits on the OS. That costs a pass
    /// over the buffer here, on the control side. One recorder makes many
    /// takes, each of whatever tap it is told: [`record`](Recorder::record)
    /// one, [`stop`](Recorder::stop) it or let it fill, and ask
    /// [`is_recording`](Recorder::is_recording) to find out it has ended.
    /// There is one at a time: refused as `RecorderInUse` until the last
    /// one is dropped, and as `OverBudget` if the ledger cannot afford it.
    pub fn recorder(&self, frames: usize) -> Result<Recorder, ControlError> {
        if self.0.recorder_held.swap(true, Ordering::Relaxed) {
            return Err(ControlError::RecorderInUse);
        }
        let samples = frames.saturating_mul(Renderer::CHANNELS);
        let mut block = match self.0.ledger.alloc(samples) {
            Ok(block) => block,
            Err(error) => {
                self.free_recorder();
                return Err(error.into());
            }
        };
        // The renderer writes it; touch every page here so it never waits.
        block.prefault();
        let audio =
            AudioBuffer::new(ChannelLayout::Stereo, block).expect("an empty block is whole frames");
        Ok(Recorder::new(self.clone(), Take::new(audio)))
    }

    /// Adds an input: the ability to put audio into the renderer. Mints a
    /// ring per sound, sends the reading ends to the render side, and
    /// returns the control and the feed. The [`Input`] is yours to hold:
    /// its [`sounds`](Input::sounds) are what you monitor, and dropping
    /// it removes the input. The [`InputFeed`] goes to whoever owns the
    /// capture thread, to feed from a device's input callback. Nothing is
    /// heard until a sound is monitored.
    ///
    /// An input is shaped like the device it comes from. `layout` says
    /// what each channel of the device's interleaved frames means, in
    /// order: a `Stereo` is the next two channels heard left and right, a
    /// `Mono` is the next one heard on both sides. The frames fed are as
    /// wide as the layout adds up to, 1 to `MAX_INPUT_CHANNELS`, exactly
    /// as the device delivered them. Each entry is a sound of its own,
    /// its own ring split out of the device frames by [`InputFeed::feed`],
    /// so the render side sees mono and stereo sounds and nothing wider.
    /// The layout is fixed for the input's life: a device that is to
    /// deliver other channels is closed and reopened, and so is its input.
    /// Up to the renderer's `input_sounds` sounds at once across every input; an
    /// input whose sounds would not all fit is refused with `InputsFull`
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
        layout: &InputLayout,
        period: usize,
    ) -> Result<(Input, InputFeed), ControlError> {
        let width = layout.width();
        if !(1..=MAX_INPUT_CHANNELS).contains(&width) {
            return Err(ControlError::BadInputWidth(width));
        }
        if !(1..=MAX_PERIOD_FRAMES).contains(&period) {
            return Err(ControlError::BadInputPeriod(period));
        }
        // A sound per entry. The capture callback writes frames in
        // through the feed, the renderer reads them out of the rings on
        // the audio thread. One command per sound; all of
        // them go or none does. The sound ids a refused add minted are
        // burnt, and nothing minds.
        let count = layout.entries().len();
        let first = self.mint(count);
        let sounds: Vec<SoundId> = (0..count as u64).map(|n| SoundId(first.0 + n)).collect();
        let (feed, rings) = InputFeed::new(layout, period);
        self.add_input(&sounds, rings)?;
        Ok((Input::new(&sounds, self.clone()), feed))
    }

    /// Stops the sound `id` if it is still playing.
    pub(crate) fn stop_sound(&self, id: SoundId) -> Result<(), ControlError> {
        self.send(SoundCommand::<Arc<AudioBuffer>>::Remove(id))
    }

    /// `count` fresh, consecutive sound ids; the first is returned.
    fn mint(&self, count: usize) -> SoundId {
        SoundId(self.0.next_sound.fetch_add(count as u64, Ordering::Relaxed))
    }

    /// Adds the sounds of an input, the sound `sounds[n]` reading
    /// `rings[n]`, none of them heard yet (at gain 0), counting each
    /// as held. All are added or none is: refused as `InputsFull`
    /// if they would not all fit beside the input sounds already held, so
    /// an input already heard is never cut by another being added, and as
    /// `CommandsFull` with nothing sent and nothing counted if the ring has
    /// no room for every one.
    fn add_input(&self, sounds: &[SoundId], rings: Vec<InputRing>) -> Result<(), ControlError> {
        debug_assert_eq!(sounds.len(), rings.len(), "a sound per ring");
        let count = sounds.len();
        self.0
            .input_sounds
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                (held + count <= self.0.input_capacity).then_some(held + count)
            })
            .map_err(|_| ControlError::InputsFull)?;
        let sent = self.send_all(sounds.iter().zip(rings).map(|(&id, from)| {
            SoundCommand::Add {
                id,
                from,
                gain: 0.0,
            }
            .into()
        }));
        if sent.is_err() {
            self.0.input_sounds.fetch_sub(count, Ordering::Relaxed);
        }
        sent
    }

    /// Removes the input sounds `sounds`, dropping their rings, and
    /// stops counting them as held. All or none, as
    /// [`add_input`](Self::add_input); a refused remove keeps them counted.
    pub(crate) fn remove_input(&self, sounds: &[SoundId]) -> Result<(), ControlError> {
        self.send_all(
            sounds
                .iter()
                .map(|&id| SoundCommand::<InputRing>::Remove(id).into()),
        )?;
        self.0
            .input_sounds
            .fetch_sub(sounds.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Hears the input sound `id` through the output, or stops hearing it:
    /// its gain set to 1 or 0. It keeps running either way, so its ring
    /// stays drained.
    pub(crate) fn set_monitored(&self, id: SoundId, monitored: bool) -> Result<(), ControlError> {
        let level = if monitored { 1.0 } else { 0.0 };
        self.send(SoundCommand::<InputRing>::Set {
            id,
            param: SoundParam::Gain(level),
        })
    }

    /// How many input sounds are held.
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
        self.channel()
            .commands
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
        let mut channel = self.channel();
        if channel.commands.slots() < commands.len() {
            return Err(ControlError::CommandsFull);
        }
        for command in commands {
            channel
                .commands
                .push(command)
                .expect("room was checked under the lock");
        }
        Ok(())
    }

    /// The take of the take `id`, in the buffer it was recorded
    /// into, once the renderer has sent it back: full or stopped. `None`
    /// while it is still recording, or once it has been taken.
    pub(crate) fn take_back(&self, id: TakeId) -> Option<AudioBuffer> {
        self.channel().takes.remove(&id)
    }

    /// Moves `audio` to the renderer to record what passes `tap` as a new
    /// take, and returns its id. Refused as `CommandsFull` if the
    /// ring has no room, with the buffer handed back.
    pub(crate) fn start_recording(
        &self,
        tap: Tap,
        audio: AudioBuffer,
    ) -> Result<TakeId, (ControlError, AudioBuffer)> {
        let mut channel = self.channel();
        if channel.commands.slots() == 0 {
            return Err((ControlError::CommandsFull, audio));
        }
        let id = TakeId(self.0.next_take.fetch_add(1, Ordering::Relaxed));
        let record = Command::Record { id, tap, audio };
        channel
            .commands
            .push(record)
            .expect("room was checked under the lock");
        Ok(id)
    }

    /// Stops the take `id` at the renderer's next period. Refused as
    /// `CommandsFull` if the ring has no room.
    pub(crate) fn stop_recording(&self, id: TakeId) -> Result<(), ControlError> {
        self.send(Command::StopRecording(id))
    }

    /// Gives up the take `id`: its take is dropped here if it is
    /// already back, or when it comes back if not, and the renderer is
    /// told to stop it. If the ring has no room for the stop, it runs
    /// until full and is dropped then.
    pub(crate) fn abandon_take(&self, id: TakeId) {
        let mut channel = self.channel();
        if channel.takes.remove(&id).is_none() {
            channel.abandoned.insert(id);
            let _ = channel.commands.push(Command::StopRecording(id));
        }
    }

    /// Lets a recorder be made again.
    pub(crate) fn free_recorder(&self) {
        self.0.recorder_held.store(false, Ordering::Relaxed);
    }

    /// The channel to the renderer, with everything it has let go of already
    /// dropped. Every way to the renderer comes through here, so every send
    /// collects first and no path can forget to: that is what keeps the
    /// release ring from filling. A panic while holding the lock leaves
    /// the channel itself intact, so a poisoned lock is taken anyway.
    fn channel(&self) -> MutexGuard<'_, RendererChannel> {
        let mut channel = self
            .0
            .channel
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        channel.collect();
        channel
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use crate::audio::{AudioBuffer, ChannelLayout};
    use crate::config::Pcm;
    use crate::project::tests::memory_project;
    use crate::project::{Project, SampleId};
    use crate::render::{Renderer, RendererSettings};
    use crate::testing::{audio, unlimited};

    /// What these tests' renderers are made with: a small period and
    /// ring, every slot.
    const SETTINGS: RendererSettings = RendererSettings {
        command_capacity: 32,
        ..RendererSettings::new(64)
    };

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        audio(
            ChannelLayout::Mono,
            &(0..n).map(|i| i as Pcm * 0.01).collect::<Vec<_>>(),
        )
    }

    /// What `ramp(n)` costs the budget.
    fn ramp_bytes(n: usize) -> usize {
        n * size_of::<Pcm>()
    }

    /// A project on in-memory storage with a ramp of `n` frames in it, and
    /// that sample's id.
    fn project_with(n: usize) -> (Project, SampleId) {
        let mut p = memory_project();
        let id = p.add_sample(&ramp(n)).unwrap();
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
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        control.play(&loaded).unwrap();

        let out = period(&mut r, 2);
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn play_is_heard_from_the_renderers_next_period() {
        let (p, id) = project_with(2);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());

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
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());

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
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        let mut last = None;
        for _ in 0..SETTINGS.command_capacity {
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
        let (_r, control) = Renderer::new(SETTINGS, unlimited());
        let too_wide = [ChannelLayout::Mono; MAX_INPUT_CHANNELS + 1];
        for bad in [&[][..], &too_wide[..]] {
            assert!(matches!(
                control.input(&InputLayout::new(bad), 64),
                Err(ControlError::BadInputWidth(w)) if w == bad.len()
            ));
        }
        for bad in [0, MAX_PERIOD_FRAMES + 1] {
            assert!(matches!(
                control.input(&InputLayout::new([ChannelLayout::Mono]), bad),
                Err(ControlError::BadInputPeriod(p)) if p == bad
            ));
        }
        assert_eq!(control.input_sounds(), 0, "a refused input is not counted");
    }

    #[test]
    fn each_sound_of_an_input_is_heard_on_its_own_and_all_go_when_it_is_dropped() {
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        // A device of four channels: a stereo pair, then two microphones.
        let (input, mut feed) = control
            .input(
                &InputLayout::new([
                    ChannelLayout::Stereo,
                    ChannelLayout::Mono,
                    ChannelLayout::Mono,
                ]),
                64,
            )
            .unwrap();
        let [pair, mic_a, mic_b] = input.sounds() else {
            panic!("three sounds");
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

        // The verbs are on the sound, not the control.
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

        let ids: Vec<SoundId> = input.sounds().iter().map(|s| s.id()).collect();
        drop(input);
        assert_eq!(control.input_sounds(), 0, "they are no longer counted");
        period(&mut r, 1);
        assert_eq!(r.playing(), 0, "every sound of the input is gone");
        assert!(ids.iter().all(|&s| !r.is_playing(s)));
        // The feed outlives the control harmlessly: its frames go nowhere.
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.0, 0.0]);
    }

    #[test]
    fn more_input_sounds_than_slots_are_refused_until_an_input_is_dropped() {
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        // One stereo input, then mono inputs up to the capacity.
        let (first, _first_feed) = control
            .input(
                &InputLayout::new([ChannelLayout::Stereo, ChannelLayout::Mono]),
                64,
            )
            .unwrap();
        let rest: Vec<_> = (2..SETTINGS.input_sounds)
            .map(|_| {
                control
                    .input(&InputLayout::new([ChannelLayout::Mono]), 64)
                    .unwrap()
            })
            .collect();
        assert!(matches!(
            control.input(&InputLayout::new([ChannelLayout::Mono]), 64),
            Err(ControlError::InputsFull)
        ));
        period(&mut r, 2);
        assert_eq!(r.playing(), SETTINGS.input_sounds);

        // Dropping the two-sound input makes room for two sounds, not three.
        let gone: Vec<SoundId> = first.sounds().iter().map(|s| s.id()).collect();
        drop(first);
        assert!(matches!(
            control.input(
                &InputLayout::new([
                    ChannelLayout::Mono,
                    ChannelLayout::Mono,
                    ChannelLayout::Mono
                ]),
                64
            ),
            Err(ControlError::InputsFull)
        ));
        let (next, _next_feed) = control
            .input(
                &InputLayout::new([ChannelLayout::Mono, ChannelLayout::Mono]),
                64,
            )
            .unwrap();
        period(&mut r, 2);
        assert_eq!(r.playing(), SETTINGS.input_sounds);
        assert!(gone.iter().all(|&s| !r.is_playing(s)));
        assert!(next.sounds().iter().all(|s| r.is_playing(s.id())));
        assert!(rest.iter().all(|(i, _)| r.is_playing(i.sounds()[0].id())));
    }

    #[test]
    fn a_full_ring_refuses_an_input_and_holds_no_slot() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        for _ in 0..SETTINGS.command_capacity {
            control.play(&loaded).unwrap();
        }
        assert!(matches!(
            control.input(&InputLayout::new([ChannelLayout::Stereo]), 64),
            Err(ControlError::CommandsFull)
        ));
        assert_eq!(control.input_sounds(), 0);

        // With room again the input is added, its sounds consecutive.
        period(&mut r, 4);
        let before = control.next_sound();
        let (input, _feed) = control
            .input(
                &InputLayout::new([ChannelLayout::Stereo, ChannelLayout::Mono]),
                64,
            )
            .unwrap();
        let ids: Vec<SoundId> = input.sounds().iter().map(|s| s.id()).collect();
        assert_eq!(ids, [SoundId(before), SoundId(before + 1)]);
        assert_eq!(control.next_sound(), before + 2);
        assert_eq!(control.input_sounds(), 2);
    }

    #[test]
    fn an_input_dropped_onto_a_full_ring_keeps_its_slots_until_its_feed_goes() {
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        let (input, feed) = control
            .input(&InputLayout::new([ChannelLayout::Mono]), 64)
            .unwrap();
        let sound = input.sounds()[0].id();
        period(&mut r, 1);
        while control.stop_all().is_ok() {}

        // No room for the remove: the drop cannot fail, so it degrades. The
        // reader stays and is still counted as held.
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
        let gone = p.add_sample(&ramp(100)).unwrap();
        let kept = p.add_sample(&ramp(100)).unwrap();
        let gone = p.load(gone).unwrap();
        let kept = p.load(kept).unwrap();
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
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
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        let play = control.play(&loaded).unwrap();
        period(&mut r, 4);

        control.stop(&loaded).unwrap();
        drop(loaded);
        // The sound still holds the audio until the renderer applies the
        // eviction at its next render, and sends the sound back; the audio
        // is freed when the control side drops it.
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        period(&mut r, 4);
        assert!(!r.is_playing(play.id()));
        assert_eq!(
            p.ledger().in_use(),
            ramp_bytes(100),
            "not on the audio thread"
        );
        control.collect();
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn stop_on_a_full_ring_is_refused_and_the_sample_is_untouched() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (_r, control) = Renderer::new(SETTINGS, unlimited());
        for _ in 0..SETTINGS.command_capacity {
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
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        let play = control.play(&loaded).unwrap();
        period(&mut r, 4);

        drop(loaded);
        // Nothing was sent: the sound keeps the audio alive and plays on.
        period(&mut r, 4);
        assert!(r.is_playing(play.id()));
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        // The audio is let go when the sound is, and freed on the next
        // send, here the next stop.
        control.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        control.stop_all().unwrap();
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn a_take_sent_back_is_kept_for_its_owner_by_take() {
        let (mut r, control) = Renderer::new(SETTINGS, unlimited());
        let id = TakeId(7);
        let audio = crate::testing::take_buffer(1);
        let tap = Tap::MainOut;
        control.send(Command::Record { id, tap, audio }).unwrap();
        assert!(control.take_back(id).is_none(), "not sent back yet");

        period(&mut r, 1);
        let take = control.take_back(id).expect("full, so sent back");
        assert_eq!(take.samples(), &[0.0, 0.0]);
        assert!(control.take_back(id).is_none(), "taken once");
    }
}
