//! Rendering: commands and resident audio in, frames of output out.
//!
//! This is the half of the engine that runs where the frames are needed.
//! Live, that is the audio thread, called once per period from a device
//! callback with a hard deadline. Offline, it is a bounce looping on the
//! control thread as fast as the CPU goes. Both get the same frames,
//! because nothing here depends on where it runs: no allocation, no lock,
//! no disk, no error path, no clock. Frame counts only.
//!
//! ```text
//! control side                          render side
//! (the engine)  ── Command ring ──▶     Renderer    commands in, a period out
//!                                         Voice     one sample playing
//! ```
//!
//! The dependency points one way. This module defines the [`Command`]s it
//! accepts and the [`VoiceId`]s that name voices; the control side imports
//! them. Nothing here knows what a project, a device or resident audio is:
//! audio arrives inside a command as an `Arc<AudioBuffer>`, and a voice
//! holds its clone until it finishes, so freeing happens only where the
//! control side lets go of the last reference.
//!
//! Outside the crate, all of this is one type: [`Output`], which the engine
//! mints and the caller drives. The renderer behind it is the engine's
//! business.

pub mod command;
pub mod output;
pub mod renderer;
pub mod voice;

pub use command::{Command, VoiceId};
pub use output::Output;
pub use renderer::Renderer;
