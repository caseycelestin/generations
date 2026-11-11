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
//! its [`SoundId`], a recording by its [`RecordingId`], a buffer by its
//! [`BufferAddress`].
//!
//! One thing goes the other way: every sound the renderer lets go of
//! comes back as a [`Released`], so its last free happens on the control
//! side and never on the audio thread.

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::render::dsp::{InputReader, Recorder, SampleReader};
use crate::render::sound::Sound;

/// What the handle can ask of the renderer.
///
/// An input `Add` carries a whole `InputReader`, which is wide; every
/// command is a slot of that size on a ring allocated once at `new`, so
/// the size is paid there and nothing else.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// An operation on one sample sound.
    Sample(SoundCommand<SampleReader>),
    /// An operation on one input sound.
    Input(SoundCommand<InputReader>),
    /// Remove every sample sound. Inputs are left alone.
    StopAllSamples,
    /// Remove every sample sound playing the buffer at the address, so
    /// the control side can let go of the last reference to it.
    Evict(BufferAddress),
    /// Keep the main out, as heard, in `recorder` from the next period
    /// on, as the recording `id`. A recorder already held is let go of.
    #[cfg_attr(not(test), expect(dead_code, reason = "no control-side sender yet"))]
    Record { id: RecordingId, recorder: Recorder },
}

impl From<SoundCommand<SampleReader>> for Command {
    fn from(command: SoundCommand<SampleReader>) -> Self {
        Self::Sample(command)
    }
}

impl From<SoundCommand<InputReader>> for Command {
    fn from(command: SoundCommand<InputReader>) -> Self {
        Self::Input(command)
    }
}

/// A sound the renderer has let go of, on its way back to the control
/// side to be dropped there. It may hold the last reference to a sample's
/// audio or the last end of an input's ring, and freeing either is a call
/// into the allocator, and for a large buffer into the kernel, that has
/// no bound on how long it takes. Nothing reads it; arriving is its job.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
#[expect(dead_code, reason = "held only to be dropped on the control side")]
pub enum Released {
    /// A sample sound: finished, removed, stolen or evicted.
    Sample(Sound<SampleReader>),
    /// An input sound: finished, removed, or refused for want of room.
    Input(Sound<InputReader>),
    /// The recorder of the recording `id`, with what it kept: full, or
    /// replaced by another.
    Recorder { id: RecordingId, recorder: Recorder },
}

impl From<Sound<SampleReader>> for Released {
    fn from(sound: Sound<SampleReader>) -> Self {
        Self::Sample(sound)
    }
}

impl From<Sound<InputReader>> for Released {
    fn from(sound: Sound<InputReader>) -> Self {
        Self::Input(sound)
    }
}

/// What every sound has done to it, whatever its source `S`.
#[derive(Debug)]
pub enum SoundCommand<S> {
    /// Hold `source` as the sound `id`, heard from the start at `gain`,
    /// a linear level. The source is built on the control side and ready
    /// to run; this is its only crossing.
    Add { id: SoundId, source: S, gain: f32 },
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

/// Identity of one recording: one use of a recorder, from the command
/// that starts it to the take that comes back. Minted by the control side,
/// so it knows whose take it is when the recorder returns; the renderer
/// carries it and never reads it.
///
/// Never reused, so a take that comes back after its owner has gone names
/// no one and can be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordingId(pub u64);

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
