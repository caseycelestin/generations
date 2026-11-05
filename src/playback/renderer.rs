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
//! This file holds the shape so far; behaviour comes next.

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
#[allow(dead_code)] // read by render, which comes next
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
}
