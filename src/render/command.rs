//! What the control side can ask of the renderer.
//!
//! A [`Command`] is a method call made into a value so it can cross to the
//! audio thread on the ring and be applied there at the next period. The
//! renderer defines what it will accept; its handle builds and sends them.
//!
//! Commands are low level: operations on what the renderer holds, never
//! intents. Add a source, remove it, set one of its parameters. What an
//! operation means to a user (monitoring an input, muting a pad, a level)
//! is the control side's, on its handles, which turn it into these. The
//! audio thread applies values and decides nothing, so a new feature on a
//! handle is rarely a new command here.
//!
//! The renderer holds samples and inputs apart, so the operations every
//! sound has, [`SoundCommand`], say which kind first. A [`SoundId`] names
//! one sound of one kind, so a command meant for one cannot land on the
//! other. The rest are the few operations that need what only the render
//! side knows, which sounds are still playing and what they play, and are
//! for samples alone.
//!
//! The two sides name things to each other with the ids here: a sound by
//! its [`SoundId`], a take by its [`TakeId`], a buffer by its
//! [`BufferAddress`].
//!
//! One thing goes the other way: every sound the renderer lets go of
//! comes back as a [`Released`], so its last free happens on the control
//! side and never on the audio thread.
//!
//! Only plain things cross, both ways: audio, the ends of rings, ids and
//! values. What the renderer runs, its dsp pieces, it builds from what a
//! command carries and takes apart again before anything goes back, so
//! neither side ever holds the other's types.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::config::Pcm;

/// What the handle can ask of the renderer.
///
/// An input `Add` carries a whole `InputRing`, which is wide; every
/// command is a slot of that size on a ring allocated once at `new`, so
/// the size is paid there and nothing else.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// An operation on one sample sound, made from a loaded sample's audio.
    Sample(SoundCommand<Arc<AudioBuffer>>),
    /// An operation on one input sound, made from its ring.
    Input(SoundCommand<InputRing>),
    /// Remove every sample sound. Inputs are left alone.
    StopAllSamples,
    /// Remove every sample sound playing the buffer at the address, so
    /// the control side can let go of the last reference to it.
    Evict(BufferAddress),
    /// Record what passes `tap` into `audio` from the next period on, as
    /// the take `id`. The buffer is paid for on the control side and
    /// moves here for the take; whatever it held is forgotten. A take
    /// already running at that tap ends and goes back.
    Record {
        id: TakeId,
        tap: Tap,
        audio: AudioBuffer,
    },
    /// Let go of the take `id` with what it has kept so far, if it
    /// is still held. One that has already filled names nothing.
    StopRecording(TakeId),
}

impl From<SoundCommand<Arc<AudioBuffer>>> for Command {
    fn from(command: SoundCommand<Arc<AudioBuffer>>) -> Self {
        Self::Sample(command)
    }
}

impl From<SoundCommand<InputRing>> for Command {
    fn from(command: SoundCommand<InputRing>) -> Self {
        Self::Input(command)
    }
}

/// The reading end of one input sound's ring and what is on it: frames
/// of `layout`, mono or stereo, the capture callback writes at the other
/// end. The ring's capacity is whole frames, so a frame never wraps.
#[derive(Debug)]
pub struct InputRing {
    pub frames: Consumer<Pcm>,
    pub layout: ChannelLayout,
}

/// What the renderer has let go of, on its way back to the control side.
/// What a sound was made from goes back to be dropped there: it may be
/// the last reference to a sample's audio or the last end of an input's
/// ring, and freeing either is a call into the allocator, and for a large
/// buffer into the kernel, that has no bound on how long it takes.
/// Nothing reads it; arriving is its job. A take goes back to its owner.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
#[expect(dead_code, reason = "held only to be dropped on the control side")]
pub enum Released {
    /// The audio of a sample sound: finished, removed, stolen or evicted.
    Sample(Arc<AudioBuffer>),
    /// The ring of an input sound: finished, removed, or refused for want
    /// of room.
    Input(InputRing),
    /// The take of the take `id`: the buffer it was recorded into,
    /// holding what it kept. Full, stopped, or ended by another take at
    /// its tap.
    Take { id: TakeId, audio: AudioBuffer },
}

/// What every sound has done to it, whatever it is made from, `M`.
#[derive(Debug)]
pub enum SoundCommand<M> {
    /// Hold a sound made `from` this as the sound `id`, heard from the
    /// start at `gain`, a linear level. The renderer builds what plays it;
    /// building only moves what is given, so nothing is allocated there.
    Add { id: SoundId, from: M, gain: f32 },
    /// Let go of the sound `id`, if it is still held.
    Remove(SoundId),
    /// Change one parameter of the sound `id`, if it is still held.
    Set { id: SoundId, param: SoundParam },
}

/// A parameter of a sound, with the value to move it to. The renderer
/// moves to it smoothly, so a change never clicks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundParam {
    /// Level as a linear factor: 0 is silent, 1 is as the source gives it.
    Gain(f32),
}

/// A point in the signal path where a recorder records: what passes
/// there is kept, and goes on where it was going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tap {
    /// The main out after the clip: exactly what is heard.
    MainOut,
}

/// Identity of one take: one use of a recorder's buffer, from the
/// command that starts it to the take that comes back. Minted by the
/// control side, so it knows whose take it is when it returns; the
/// renderer carries it and never reads it.
///
/// Never reused, so a take that comes back after its owner has gone names
/// no one and can be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TakeId(pub u64);

/// Identity of one sound: one instance of something heard, a sample
/// played once or one sound of an input. Minted by the control side, in order,
/// so the lowest id among the sounds is the oldest.
///
/// Never reused, so a stop that arrives after its sound was stolen or
/// finished names nothing and does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoundId(pub u64);

/// Where a resident buffer lives, as a number.
///
/// The render side never learns a sample id, and a raw pointer cannot
/// cross to another thread, so a buffer is named to the renderer by its
/// address. Two live `Arc<AudioBuffer>`s never share one, and the
/// eviction is sent while the control side still holds the buffer, so
/// the address is still that buffer's when the command is applied: any
/// later buffer at the same address is started by a play that is behind
/// it on the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferAddress(usize);

impl BufferAddress {
    /// The address of the buffer `audio` points at.
    pub fn of(audio: &Arc<AudioBuffer>) -> Self {
        Self(Arc::as_ptr(audio) as usize)
    }
}
