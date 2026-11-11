//! A sound the caller holds: one play of a sample, and what can be done
//! to it while it plays.
//!
//! Playing a [`Sample`] returns a [`SampleSound`]. The sound's verbs are
//! on the sound: `stop` is `sound.stop()`, not a method elsewhere that
//! takes an id. The id the renderer knows the sound by is inside the handle and
//! never leaves it; the only way to get a `SampleSound` is to play one, so a
//! `SampleSound` always names a sound that was started. Dropping the handle
//! changes nothing: the sound plays to its end. It owns nothing; the
//! audio is the [`Sample`]'s and the place it plays in is the renderer's.
//!
//!
//! [`Sample`]: crate::project::sample::Sample

use crate::control::Control;
use crate::control::ControlError;
use crate::render::command::SoundId;

/// One play of a sample. Drop it and the sound plays on to its end;
/// `stop` it to cut it.
#[derive(Debug)]
pub struct SampleSound {
    id: SoundId,
    control: Control,
}

impl SampleSound {
    /// The sound `id`, playing on `renderer`.
    pub(crate) fn new(id: SoundId, control: Control) -> Self {
        Self { id, control }
    }

    /// The id the renderer knows this sound by.
    #[cfg(test)]
    pub(crate) fn id(&self) -> SoundId {
        self.id
    }

    /// Stops the sound if it is still playing, at the output's next
    /// period. One that has finished or been stolen is left alone, so
    /// stopping twice is harmless. Refused as `CommandsFull` if the ring
    /// has no room.
    pub fn stop(&self) -> Result<(), ControlError> {
        self.control.stop_sound(self.id)
    }
}
