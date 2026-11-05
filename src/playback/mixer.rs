//! The voices summed into a period.
//!
//! The [`Mixer`] lives on the real-time thread. Each period the device
//! hands it a stereo `f32` slice; the mixer takes the commands that have
//! arrived, zeroes the slice, lets every playing voice add itself, clips
//! the sum once, and returns. Nothing in here may allocate, lock, block or
//! fail.
//!
//! Commands come over a lock-free ring from the control thread. The mixer
//! owns the consuming end; whoever owns the producing end is the only
//! other code that knows the mixer exists.
//!
//! This file holds the shape so far; rendering comes next.

use std::sync::Arc;

use rtrb::Consumer;

use crate::audio::AudioBuffer;
use crate::config::VOICE_COUNT;
use crate::playback::voice::Voice;

/// Identity of one started sound, minted on the control thread.
///
/// Never reused, so a stop that arrives after its voice was stolen or
/// finished names nothing and does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(pub u64);

/// What the control thread can ask of the mixer.
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

/// One occupied slot in the voice table.
#[derive(Debug)]
#[allow(dead_code)] // read by render, which comes next
struct Slot {
    id: VoiceId,
    /// When this voice started, counted in starts: the lowest is the
    /// oldest, and the oldest is what a full table steals.
    started: u64,
    voice: Voice,
}

/// The voices summed into a period.
#[derive(Debug)]
#[allow(dead_code)] // read by render, which comes next
pub struct Mixer {
    /// The consuming end of the command ring.
    commands: Consumer<Command>,
    /// The voice table: fixed size, so the work in one period is bounded.
    slots: [Option<Slot>; VOICE_COUNT],
    /// How many voices have ever started; stamps each new slot.
    starts: u64,
}

impl Mixer {
    /// A silent mixer that takes its commands from `commands`.
    pub fn new(commands: Consumer<Command>) -> Self {
        Self {
            commands,
            slots: [const { None }; VOICE_COUNT],
            starts: 0,
        }
    }
}
