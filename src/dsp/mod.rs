//! Audio processing: commands and resident audio in, frames of output out.
//!
//! This is the half of the engine that runs where the frames are needed,
//! on a thread the engine does not own.
//! Live, that is the audio thread, called once per period from a device
//! callback with a hard deadline. Offline, it is a bounce looping on the
//! control thread as fast as the CPU goes. Both get the same frames,
//! because nothing here depends on where it runs: no allocation, no lock,
//! no disk, no error path, no clock. Frame counts only.
//!
//! ```text
//! control side                          render side
//! (the engine)  ── Command ring ──▶     Renderer    the wiring: commands in, a period out
//!                                         VoiceTable    the samples playing, one source
//!                                           Voice   one sample playing
//! ```
//!
//! The dependency points one way. This module defines the [`Command`]s it
//! accepts and the [`VoiceId`]s that name voices; the control side imports
//! them. Nothing here knows what a project, a device or resident audio is:
//! audio arrives inside a command as an `Arc<AudioBuffer>`, and a voice
//! holds its clone until it finishes or is told to let go. The control
//! side names a buffer to this side by its address, never by what it is.
//!
//! Outside the crate, all of this is one type with one method: the
//! [`Renderer`], which the engine mints as its output and the caller
//! drives with `render`. What it holds is the engine's business.

pub mod command;
pub mod renderer;
pub mod voice;
pub mod voice_table;

pub use command::{BufferAddr, Command, VoiceId};
pub use renderer::Renderer;
