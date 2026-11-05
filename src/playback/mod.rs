//! Playback: the audio a player can hear, and how it is heard.
//!
//! Two halves, one rule. The control half, where the engine runs, loads
//! audio into RAM and asks for sounds to start and stop. The rendering
//! half turns what is resident into output, one period at a time, and is
//! real-time safe: no allocation, no lock, no disk, no error path, no
//! clock. Live, an audio device drives it from its callback and the two
//! halves are two threads. Offline, a bounce drives it in a loop on the
//! control thread and gets the same frames.
//!
//! ```text
//! control half                       rendering half
//! ResidentSet   audio in RAM         Voice       one sample playing
//!               under a budget       Renderer    commands in, a period out
//!                         ── Command ring ──▶
//! ```
//!
//! Commands cross over a lock-free ring, one way. Audio crosses as
//! `Arc<AudioBuffer>`: the resident set holds one reference, a playing
//! voice holds a clone, and freeing happens only where the control half
//! lets go of the last one. The [`Device`] is what makes the rendering
//! half a second thread: it owns the renderer and calls it from the audio
//! callback.

pub mod device;
pub mod renderer;
pub mod resident;
pub mod voice;

use thiserror::Error;

use crate::config::COMMAND_CAPACITY;
use crate::project::SampleId;

pub use device::{Device, DeviceError};
pub use renderer::{Command, Renderer, VoiceId};
pub use resident::ResidentSet;
pub use voice::Voice;

/// Reasons playback cannot do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlaybackError {
    /// Loading this audio would put resident audio over the budget.
    #[error(
        "loading {requested} bytes would put resident audio at {} of a {budget} byte budget",
        resident + requested
    )]
    OverBudget {
        /// Bytes the refused audio would have cost.
        requested: usize,
        /// Bytes resident at the time.
        resident: usize,
        /// The budget this set was built with.
        budget: usize,
    },
    /// The sample exists but is not loaded, so there is nothing resident
    /// to play. Load it first.
    #[error("sample {} is not loaded", .0.0)]
    NotLoaded(SampleId),
    /// The command ring was full; the command was dropped.
    #[error("the renderer has not taken the last {COMMAND_CAPACITY} commands")]
    CommandsFull,
    /// The audio device could not be opened or started.
    #[error(transparent)]
    Device(#[from] DeviceError),
}
