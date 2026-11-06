//! Playback: the control side of hearing a sample.
//!
//! This is what the engine holds. It loads audio into RAM under a budget
//! and asks for sounds to start and stop, by sending commands over a
//! lock-free ring to a [`Renderer`](crate::render::Renderer), which lives
//! in [`render`](crate::render) and turns them into frames wherever it is
//! driven.
//!
//! ```text
//! control side                       render side
//! Playback      what the engine      Renderer    (see crate::render)
//!   ResidentSet   holds: audio in
//!   Producer      RAM and the ring's
//!                 sending end
//!                         ── Command ring ──▶
//! ```
//!
//! Playback never touches a device. `Playback::new` hands the renderer
//! back, and whoever called decides what drives it: the engine puts it on
//! a [`Device`](crate::device::Device), a bounce would loop over it, a test
//! calls it by hand.
//!
//! Audio crosses as `Arc<AudioBuffer>`: the resident set holds one
//! reference, a playing voice holds a clone, and freeing happens only where
//! the control side lets go of the last one.

pub mod control;
pub mod resident;

use thiserror::Error;

use crate::config::COMMAND_CAPACITY;
use crate::project::SampleId;

pub use control::Playback;
pub use resident::ResidentSet;

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
}
