//! The control side's handle on the renderer: what you hold here to make
//! the renderer, on its own thread, do something.
//!
//! The engine mints the renderer and the command ring between them, and
//! wraps the ring's sending end in a [`RendererHandle`]. The handle is
//! cheap to clone, and the engine gives one to every [`LoadedSample`],
//! [`Sound`] and [`Input`] it hands out, so each can act on the renderer
//! without going back through the engine. Its methods are the verbs the
//! renderer
//! accepts, `play`, `stop`, `monitor`, named as such; building the
//! [`Command`] that carries each one is this type's job and nobody
//! else's, so the command vocabulary is private between here and the
//! renderer.
//!
//! Every send goes through one lock, on the control side only. The
//! renderer's end of the ring is unchanged and never blocks, and the
//! renderer never sees a handle, only the commands. Nothing that runs on
//! the audio or capture thread may hold one of these: it has a mutex.
//!
//! The handle also mints [`SoundId`]s, because a sound's id has to be in
//! the command that starts it, and whoever starts sounds must agree on
//! the names: a `LoadedSample` plays without the engine, and the id it plays as
//! must not collide with one minted for an input. One counter behind the
//! same `Arc`, only going up, so no id is ever reused. A command refused
//! for want of room leaves its id burnt; ids are plentiful and nothing
//! reads the gaps.
//!
//! [`LoadedSample`]: crate::loaded_sample::LoadedSample
//! [`Sound`]: crate::sound::Sound
//! [`Input`]: crate::input::Input

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::Producer;

use crate::audio::AudioBuffer;
use crate::config::MAX_INPUT_SOUNDS;
use crate::dsp::{BufferAddr, InputReader, SoundId};
use crate::engine::EngineError;
use crate::renderer::command::{Command, InputCommand, SampleCommand};

/// The control side's handle on the renderer. One per engine, cloned into
/// every handle that acts on what is heard.
#[derive(Debug, Clone)]
pub(crate) struct RendererHandle(Arc<Shared>);

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

impl RendererHandle {
    /// A handle sending over `commands`, minting sound ids from zero.
    pub(crate) fn new(commands: Producer<Command>) -> Self {
        Self(Arc::new(Shared {
            commands: Mutex::new(commands),
            next_sound: AtomicU64::new(0),
            input_sounds: AtomicUsize::new(0),
        }))
    }

    /// Starts `audio` from its first frame as a new sound, and returns its
    /// id. Refused as `CommandsFull` if the ring has no room.
    pub(crate) fn play(&self, audio: Arc<AudioBuffer>) -> Result<SoundId, EngineError> {
        let id = self.mint(1);
        self.send(SampleCommand::Play { id, audio })?;
        Ok(id)
    }

    /// Stops the sample `id` if it is still playing.
    pub(crate) fn stop(&self, id: SoundId) -> Result<(), EngineError> {
        self.send(SampleCommand::Stop(id))
    }

    /// Stops every sample. Inputs are untouched.
    pub(crate) fn stop_all(&self) -> Result<(), EngineError> {
        self.send(SampleCommand::StopAll)
    }

    /// Stops every sound playing the buffer at `addr`, so its last
    /// reference can be let go on the control side.
    pub(crate) fn evict(&self, addr: BufferAddr) -> Result<(), EngineError> {
        self.send(SampleCommand::Evict(addr))
    }

    /// `count` fresh, consecutive sound ids; the first is returned. For an
    /// input, whose sources need their ids before their rings are made.
    pub(crate) fn mint(&self, count: usize) -> SoundId {
        SoundId(self.0.next_sound.fetch_add(count as u64, Ordering::Relaxed))
    }

    /// Adds the sources of an input, the sound `sounds[n]` reading
    /// `readers[n]`, none of them heard yet, taking a slot of the input
    /// mixer for each. All are added or none is: refused as `InputsFull`
    /// if they would not all fit beside the input sounds already held, so
    /// an input already heard is never cut by another being added, and as
    /// `CommandsFull` with nothing sent and no slot taken if the ring has
    /// no room for every one.
    pub(crate) fn add_input(
        &self,
        sounds: &[SoundId],
        readers: Vec<InputReader>,
    ) -> Result<(), EngineError> {
        debug_assert_eq!(sounds.len(), readers.len(), "a sound per reader");
        let count = sounds.len();
        self.0
            .input_sounds
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                (held + count <= MAX_INPUT_SOUNDS).then_some(held + count)
            })
            .map_err(|_| EngineError::InputsFull)?;
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
    pub(crate) fn remove_input(&self, sounds: &[SoundId]) -> Result<(), EngineError> {
        self.send_all(sounds.iter().map(|&id| InputCommand::Remove(id).into()))?;
        self.0
            .input_sounds
            .fetch_sub(sounds.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Hears the input sound `id` through the output, or stops hearing it.
    pub(crate) fn monitor(&self, id: SoundId, heard: bool) -> Result<(), EngineError> {
        self.send(if heard {
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
    fn send(&self, command: impl Into<Command>) -> Result<(), EngineError> {
        self.lock()
            .push(command.into())
            .map_err(|_| EngineError::CommandsFull)
    }

    /// Pushes every command of `commands`, or none of them: refused as
    /// `CommandsFull` with nothing sent if the ring has no room for all.
    /// The lock is held across the check and the pushes, so no other
    /// holder can take the room in between.
    fn send_all(
        &self,
        commands: impl ExactSizeIterator<Item = Command>,
    ) -> Result<(), EngineError> {
        let mut ring = self.lock();
        if ring.slots() < commands.len() {
            return Err(EngineError::CommandsFull);
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
